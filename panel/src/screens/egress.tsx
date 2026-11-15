// «Выход в интернет» (docs/plans/own-server.md, этап 3): кто из DuoWave ходит в интернет напрямую, а кто — через ваш
// сервер из приложения «VPN». Сверху — простой режим (три кнопки), сервер по умолчанию и строка итога; ниже —
// свёрнутая точная настройка: таблица по разделам, у строки маршрут («Авто» → «Напрямую» → подключения), откуда
// решение, «новое», «Нужно подключение», «если сервер не отвечает». Решает шлюз (src/egress) — экран только
// показывает и меняет его правила. Страны — `Intl.DisplayNames`, флаги — эмодзи.

import { useEffect, useMemo, useState } from 'preact/hooks';

import { EgressConnection, EgressMode, EgressRow, EgressSection, EgressView, api, can, problemOf } from '../api';
import { countryName, flagOf } from '../country';
import { formatList, tr, useLanguage } from '../i18n';
import { usePanel } from '../shell/context';
import { ask } from '../ui/confirm';
import './egress.css';

const MODES: EgressMode[] = ['off', 'blocked', 'all'];
const SECTIONS: EgressSection[] = ['alice', 'brain', 'voice', 'mcp', 'apps', 'own', 'other'];
/** Как часто экран перечитывает маршруты, пока открыт: подключение могло перестать отвечать. */
const REFRESH_MS = 10_000;

function connectionOf(view: EgressView, id: string | undefined): EgressConnection | undefined {
    return id ? view.connections.find((one) => one.id === id) : undefined;
}

/** «🇳🇱 Мой VPN»: флаг страны выхода и имя подключения. */
function connectionLabel(connection: EgressConnection | undefined, id: string): string {
    if (!connection) return id;
    return `${flagOf(connection.country)} ${connection.name}`.trim();
}

/** Что сейчас с строкой — словами. */
function decisionText(view: EgressView, row: EgressRow, locale: string): string {
    const decision = row.decision;
    switch (decision.kind) {
        case 'direct':
            return tr.egress.decisionDirect;
        case 'via': {
            const connection = connectionOf(view, decision.connection);
            return tr.egress.decisionVia(flagOf(connection?.country), connection?.name ?? decision.connection);
        }
        case 'down':
            return tr.egress.decisionDown(connectionLabel(connectionOf(view, decision.connection), decision.connection));
        case 'need': {
            const countries = formatList(decision.countries.map((code) => `${flagOf(code)} ${countryName(code, locale)}`));
            return decision.wait ? tr.egress.decisionNeed(countries) : tr.egress.decisionNeedDirect(countries);
        }
    }
}

/** Откуда решение — мелко под маршрутом. */
function whyText(row: EgressRow): string {
    const why = row.why;
    switch (why.kind) {
        case 'noConnections':
            return tr.egress.whyNoConnections;
        case 'off':
            return tr.egress.whyOff;
        case 'all':
            return tr.egress.whyAll;
        case 'own':
            return tr.egress.whyOwn;
        case 'noCountry':
            return tr.egress.whyNoCountry;
        case 'homeWorks':
            return tr.egress.whyHomeWorks;
        case 'catalog':
            return tr.egress.whyCatalog(why.country);
        case 'learned':
            return tr.egress.whyLearned;
        case 'noData':
            return tr.egress.whyNoData;
    }
}

/** Следующий маршрут строки по нажатию: Авто → Напрямую → каждое подключение → Авто. */
function nextRoute(view: EgressView, route: string): string {
    const order = ['auto', 'direct', ...view.connections.map((one) => one.id)];
    const at = order.indexOf(route);
    return order[(at + 1) % order.length];
}

function routeLabel(view: EgressView, route: string): string {
    if (route === 'auto') return tr.egress.routeAuto;
    if (route === 'direct') return tr.egress.routeDirect;
    return connectionLabel(connectionOf(view, route), route);
}

/** Строка итога: через какое подключение что идёт, что напрямую, что ждёт. */
function summary(view: EgressView): string[] {
    const names = (rows: EgressRow[]): string => {
        const shown = rows.slice(0, 5).map((row) => row.name);
        if (rows.length > 5) shown.push(tr.egress.andMore(rows.length - 5));
        return formatList(shown);
    };
    const lines: string[] = [];
    const via = new Map<string, EgressRow[]>();
    const down: EgressRow[] = [];
    const need: EgressRow[] = [];
    const direct: EgressRow[] = [];
    for (const row of view.rows) {
        const decision = row.decision;
        if (decision.kind === 'via') via.set(decision.connection, [...(via.get(decision.connection) ?? []), row]);
        else if (decision.kind === 'down') down.push(row);
        else if (decision.kind === 'need' && decision.wait) need.push(row);
        else direct.push(row);
    }
    for (const [id, rows] of via) {
        const connection = connectionOf(view, id);
        lines.push(tr.egress.summaryVia(flagOf(connection?.country), connection?.name ?? id, names(rows)));
    }
    if (down.length) lines.push(tr.egress.summaryDown(names(down)));
    if (need.length) lines.push(tr.egress.summaryNeed(names(need)));
    if (!via.size && !down.length && !need.length) lines.push(tr.egress.summaryAllDirect);
    else if (direct.length) lines.push(tr.egress.summaryDirect(`${names(direct.filter((row) => row.section !== 'other'))}${direct.some((row) => row.section === 'other') ? `, ${tr.egress.homeAndRest}` : ''}`.replace(/^, /, '')));
    return lines;
}

function RowLine({ view, row, mayEdit, onChange }: { view: EgressView; row: EgressRow; mayEdit: boolean; onChange: (change: { route?: string; onDown?: 'wait' | 'direct' | null }) => void }) {
    const locale = useLanguage();
    const refuses = row.decision.kind === 'down' || (row.decision.kind === 'need' && row.decision.wait);
    const hosts = row.hosts.length === 0 ? (row.viaEntry ? tr.egress.programs : '') : row.hosts.length <= 2 ? row.hosts.join(', ') : tr.egress.hostsCount(row.hosts.length);
    return (
        <li class="egress-row">
            <div class="flex flex-col gap-2 min-w-0">
                <span class="t-body">
                    {row.name}
                    {row.new && <span class="pill pill-own ml-6">{tr.egress.isNew}</span>}
                    {row.keyed && <span class="t-meta ml-6">{tr.egress.keyed}</span>}
                </span>
                {hosts && <span class="t-meta truncate">{hosts}</span>}
                <span class={`t-caption ${refuses ? 'c-dot-offline' : ''}`}>
                    {tr.egress.now} {decisionText(view, row, locale)} · {whyText(row)}
                </span>
                {view.connections.length > 0 ? (
                    <span class="t-meta">
                        {tr.egress.onDownLabel}{' '}
                        <button
                            class="pill pill-btn"
                            disabled={!mayEdit}
                            onClick={() => onChange({ onDown: row.onDown === 'wait' ? 'direct' : 'wait' })}
                        >
                            {row.onDown === 'wait' ? tr.egress.onDownWait : tr.egress.onDownDirect}
                        </button>
                    </span>
                ) : null}
            </div>
            <button class={`pill pill-btn ${row.route === 'auto' ? '' : 'pill-own'}`} title={tr.egress.routeHint} disabled={!mayEdit} onClick={() => onChange({ route: nextRoute(view, row.route) })}>
                {routeLabel(view, row.route)}
            </button>
        </li>
    );
}

function CustomHosts({ view, mayEdit, onSave }: { view: EgressView; mayEdit: boolean; onSave: (hosts: string[]) => void }) {
    const [text, setText] = useState('');
    const add = (): void => {
        const host = text.trim();
        if (!host) return;
        onSave([...view.custom, host]);
        setText('');
    };
    return (
        <div class="grid gap-8">
            {view.custom.length === 0 && <p class="t-caption m-0">{tr.egress.customEmpty}</p>}
            {mayEdit && (
                <div class="flex gap-8">
                    <input
                        class="field flex-1"
                        placeholder={tr.egress.customLabel}
                        aria-label={tr.egress.customLabel}
                        value={text}
                        onInput={(event) => setText((event.target as HTMLInputElement).value)}
                        onKeyDown={(event) => event.key === 'Enter' && add()}
                    />
                    <button class="w-btn" type="button" onClick={add}>
                        {tr.egress.customAdd}
                    </button>
                </div>
            )}
            {view.custom.map((host) => (
                <div key={host} class="flex items-center gap-8">
                    <span class="t-body flex-1">{host}</span>
                    {mayEdit && (
                        <button class="pill pill-btn" onClick={() => onSave(view.custom.filter((one) => one !== host))}>
                            {tr.egress.customRemove}
                        </button>
                    )}
                </div>
            ))}
        </div>
    );
}

/** «Выход в интернет» — страница «Настроек». */
export function EgressScreen() {
    const panel = usePanel();
    const mayEdit = can('settings.edit');
    const [view, setView] = useState<EgressView | null>(null);
    const [problem, setProblem] = useState<string | null>(null);
    const [busy, setBusy] = useState(false);
    useEffect(() => {
        let alive = true;
        let seen = false;
        const read = (): void => {
            api.egress().then(
                (answer) => {
                    if (!alive) return;
                    setView(answer);
                    // Открыли экран — «новое» увидено (один раз за открытие).
                    if (!seen && answer.rows.some((row) => row.new)) {
                        seen = true;
                        void api.egressSeen().catch(() => undefined);
                    }
                },
                (error) => alive && setProblem(problemOf(error)),
            );
        };
        read();
        const timer = window.setInterval(read, REFRESH_MS);
        return () => {
            alive = false;
            window.clearInterval(timer);
        };
    }, []);
    const change = async (work: () => Promise<EgressView>): Promise<void> => {
        setBusy(true);
        setProblem(null);
        try {
            setView(await work());
        } catch (error) {
            setProblem(problemOf(error));
        } finally {
            setBusy(false);
        }
    };
    const lines = useMemo(() => (view ? summary(view) : []), [view]);
    if (!view)
        return (
            <section class="card glass card-list">
                <p class="t-caption m-0">{problem ?? tr.egress.loading}</p>
            </section>
        );
    const bySection = SECTIONS.map((section) => [section, view.rows.filter((row) => row.section === section)] as const).filter(([section, rows]) => rows.length > 0 || section === 'own');
    return (
        <>
            <section class="card glass card-list" aria-label={tr.egress.title}>
                <p class="t-caption m-0">
                    {tr.egress.about}
                    {view.ownRules && <span class="pill pill-own ml-6">{tr.egress.ownRules}</span>}
                </p>
                {!view.app && (
                    <div class="flex flex-col gap-8">
                        <p class="t-caption m-0">{tr.egress.noApp}</p>
                        <div>
                            <button class="w-btn" onClick={() => panel.go('magicplace', 'store', 'own-server')}>
                                {tr.egress.openMagicPlace}
                            </button>
                        </div>
                    </div>
                )}
                {view.app && !view.anyReported && (
                    <div class="flex flex-col gap-8">
                        <p class="t-caption m-0">{tr.egress.noYes}</p>
                        <div>
                            <button class="w-btn" onClick={() => panel.go('skills', 'installed', view.app ?? undefined)}>
                                {tr.egress.openApp}
                            </button>
                        </div>
                    </div>
                )}
                {!view.home && view.mode === 'blocked' && <p class="t-caption m-0">{tr.egress.noCountry}</p>}
                <div role="radiogroup" aria-label={tr.egress.modeLabel} class="look-seg glass egress-modes" style={{ gridTemplateColumns: 'repeat(3, minmax(0, 1fr))' }}>
                    {MODES.map((mode) => (
                        <button
                            key={mode}
                            role="radio"
                            aria-checked={view.mode === mode}
                            disabled={!mayEdit || busy}
                            onClick={() => view.mode !== mode && void change(() => api.setEgress({ mode }))}
                        >
                            {mode === 'off' ? tr.egress.modeOff : mode === 'blocked' ? tr.egress.modeBlocked : tr.egress.modeAll}
                        </button>
                    ))}
                </div>
                <p class="t-meta m-0">{view.mode === 'off' ? tr.egress.modeOffSub : view.mode === 'blocked' ? tr.egress.modeBlockedSub : tr.egress.modeAllSub}</p>
                {view.connections.length > 0 && (
                    <label class="flex items-center gap-8">
                        <span class="t-meta">{tr.egress.serverLabel}</span>
                        <select
                            class="field flex-1"
                            value={view.preferred ?? ''}
                            disabled={!mayEdit || busy}
                            onChange={(event) => {
                                const id = (event.target as HTMLSelectElement).value;
                                void change(() => api.setEgress({ preferred: id || null }));
                            }}
                        >
                            <option value="">{tr.egress.serverAuto}</option>
                            {view.connections.map((connection) => (
                                <option key={connection.id} value={connection.id}>
                                    {connectionLabel(connection, connection.id)}
                                    {connection.current ? ` · ${connection.current}` : ''}
                                    {connection.up ? '' : ` · ${tr.egress.vpnDown}`}
                                </option>
                            ))}
                        </select>
                    </label>
                )}
                {lines.map((line) => (
                    <p key={line} class="t-body m-0">
                        {line}
                    </p>
                ))}
                {view.ownRules && mayEdit && (
                    <div>
                        <button
                            class="pill pill-btn"
                            disabled={busy}
                            onClick={() =>
                                void ask({ title: tr.egress.resetAsk, ok: tr.egress.resetOk }).then((yes) => (yes ? change(() => api.resetEgressRows()) : undefined))
                            }
                        >
                            {tr.egress.reset}
                        </button>
                    </div>
                )}
                {problem && (
                    <p class="t-caption m-0 c-dot-offline" role="alert">
                        {problem}
                    </p>
                )}
            </section>
            <details class="card glass card-list">
                <summary class="t-title-xs">{tr.egress.details}</summary>
                {bySection.map(([section, rows]) => (
                    <div key={section} class="grid gap-8">
                        <h3 class="t-meta m-0">{tr.egress.sections[section]}</h3>
                        {section === 'own' && <CustomHosts view={view} mayEdit={mayEdit} onSave={(hosts) => void change(() => api.setEgressCustom(hosts))} />}
                        <ul class="egress-rows">
                            {rows.map((row) => (
                                <RowLine key={row.id} view={view} row={row} mayEdit={mayEdit && !busy} onChange={(patch) => void change(() => api.setEgressRow(row.id, patch))} />
                            ))}
                        </ul>
                    </div>
                ))}
                <p class="t-caption m-0">{tr.egress.always}</p>
            </details>
        </>
    );
}

/** Строка у колонки: «VPN: 🇳🇱 модели · 🇷🇺 Алиса» / «VPN не отвечает»; нет подключений — ничего. */
export function useEgressLine(): string | null {
    const [view, setView] = useState<EgressView | null>(null);
    useEffect(() => {
        if (!can('connectors.view')) return;
        let alive = true;
        const read = (): void => {
            api.egress().then(
                (answer) => alive && setView(answer),
                () => undefined,
            );
        };
        read();
        const timer = window.setInterval(read, 30_000);
        return () => {
            alive = false;
            window.clearInterval(timer);
        };
    }, []);
    if (!view || !view.anyReported || view.mode === 'off') return null;
    if (view.rows.some((row) => row.decision.kind === 'down')) return tr.egress.vpnDown;
    const parts: string[] = [];
    for (const connection of view.connections) {
        const through = view.rows.filter((row) => row.decision.kind === 'via' && row.decision.connection === connection.id);
        if (!through.length) continue;
        const brain = through.some((row) => row.section === 'brain');
        const alice = through.some((row) => row.section === 'alice');
        const what = [brain ? tr.egress.sections.brain : null, alice ? tr.egress.sections.alice : null].filter(Boolean).join(', ') || formatList(through.slice(0, 2).map((row) => row.name));
        parts.push(`${flagOf(connection.country)} ${what}`.trim());
    }
    return parts.length ? tr.egress.vpnLine(parts.join(' · ')) : null;
}
