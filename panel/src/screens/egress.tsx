// «VPN» (docs/plans/own-server.md, этап 3; образец владельца 16.11 — телефонный VPN: «Главная · Локации · Настройки»).
// Одно место для всего VPN: шлюз решает, кто ходит через сервер (src/egress), приложение «VPN» держит подписки и
// серверы (его функции зовутся через api.callPackage). Вкладки:
// - «Главная» — большой переключатель «Выкл / Вкл», строка, кто сейчас идёт через сервер, выбранная локация,
//   «Автолокация», «Недавние»;
// - «Локации» — поиск, сортировка, все серверы всех подписок: флаг, страна и город, полоски из задержки, галочка;
// - «Настройки» — «напрямую то, что работает из страны дома», «Режим VPN для DuoWave» (все / все, кроме выбранных /
//   только выбранные — со списком тех, кто в DuoWave ходит в интернет), «Если VPN не отвечает» одной настройкой на
//   всех, страна дома, свои адреса;
// - «Подписки» — подписки и ссылки: добавить, проверить, обновить, переименовать, удалить; протоколы.
// Страны — `Intl.DisplayNames`, флаги — эмодзи. Ссылки и ключи пульт не видит: приложение отдаёт только имена,
// страны, задержку и хост подписки.

import { ComponentChildren } from 'preact';
import { useCallback, useEffect, useMemo, useState } from 'preact/hooks';

import { EgressConnection, EgressOnDownAll, EgressPick, EgressRow, EgressView, api, can, problemOf } from '../api';
import { countryName, flagOf } from '../country';
import { formatList, tr, useLanguage } from '../i18n';
import { usePanel } from '../shell/context';
import { Action } from '../ui/cards';
import { ask } from '../ui/confirm';
import { Switch } from '../ui/switch';
import { SectionTabs } from '../ui/tabs';
import './egress.css';

type Tab = 'home' | 'places' | 'settings' | 'subs';

/** Как часто экран перечитывает VPN, пока открыт; чаще — пока подписка читается или подключение запускается. */
const REFRESH_MS = 5_000;
const BUSY_MS = 1_500;
/** Сколько имён в строке «Через сервер: …», остальное — «ещё N». */
const NAMES_SHOWN = 4;
/** «Недавние» — последние выбранные руками серверы, в этом браузере. */
const RECENT_KEY = 'vpn-recent';
const RECENT_MAX = 3;

/** Протоколы программы «VPN»: имена одни на всех языках. */
const PROTOCOLS = ['VLESS + Reality', 'VLESS + XHTTP', 'Hysteria 2', 'TUIC v5', 'Shadowsocks 2022', 'Trojan', 'AnyTLS', 'WireGuard'];

/** Страны ISO 3166-1 для «Страны дома». */
const COUNTRIES =
    'AD AE AF AG AI AL AM AO AQ AR AS AT AU AW AX AZ BA BB BD BE BF BG BH BI BJ BL BM BN BO BQ BR BS BT BW BY BZ CA CC CD CF CG CH CI CK CL CM CN CO CR CU CV CW CX CY CZ DE DJ DK DM DO DZ EC EE EG EH ER ES ET FI FJ FK FM FO FR GA GB GD GE GF GG GH GI GL GM GN GP GQ GR GS GT GU GW GY HK HN HR HT HU ID IE IL IM IN IO IQ IR IS IT JE JM JO JP KE KG KH KI KM KN KP KR KW KY KZ LA LB LC LI LK LR LS LT LU LV LY MA MC MD ME MF MG MH MK ML MM MN MO MP MQ MR MS MT MU MV MW MX MY MZ NA NC NE NF NG NI NL NO NP NR NU NZ OM PA PE PF PG PH PK PL PM PN PR PS PT PW PY QA RE RO RS RU RW SA SB SC SD SE SG SH SI SJ SK SL SM SN SO SR SS ST SV SX SY SZ TC TD TF TG TH TJ TK TL TM TN TO TR TT TV TW TZ UA UG UM US UY UZ VA VC VE VG VI VN VU WF WS YE YT ZA ZM ZW'.split(
        ' ',
    );

// ─── Данные приложения «VPN» (его /call/status) ──────────────────────────────────────

interface VpnServer {
    name: string;
    country?: string | null;
    delayMs?: number | null;
    up?: boolean | null;
}

interface VpnConnection {
    id: string;
    name: string;
    country?: string | null;
    state: 'up' | 'down' | 'starting';
    mode: 'auto' | 'manual';
    choice?: string | null;
    current?: string | null;
    servers: VpnServer[];
    subscription?: { host: string; updatedAt?: number | null; reading?: boolean; error?: string | null } | null;
    skipped: Array<{ scheme: string; reason: string }>;
}

interface VpnStatus {
    connections: VpnConnection[];
    max?: number;
}

interface Recent {
    connection: string;
    name: string;
}

/** Код страны из флага в начале имени сервера: «🇳🇱 Амстердам» → NL. */
function flagCode(name: string): string | null {
    const found = name.match(/^(\p{Regional_Indicator})(\p{Regional_Indicator})/u);
    if (!found) return null;
    return String.fromCharCode(...[found[1], found[2]].map((part) => (part.codePointAt(0) ?? 0) - 0x1f1e6 + 65));
}

function withoutFlag(name: string): string {
    return name.replace(/^\p{Regional_Indicator}{2}\s*/u, '').trim();
}

/** Страна и город сервера: «Казахстан» и «Алматы»; город, что совпал со страной, не повторяется. */
function placeOf(server: VpnServer, connection: VpnConnection | undefined, locale: string): { code: string | null; country: string; city: string } {
    const code = (server.country ?? flagCode(server.name) ?? connection?.country ?? null)?.toUpperCase() ?? null;
    const city = withoutFlag(server.name);
    const country = code ? countryName(code, locale) : '';
    if (!country) return { code, country: city, city: '' };
    if (!city || city.toLowerCase() === country.toLowerCase()) return { code, country, city: '' };
    return { code, country, city };
}

/** Полоски сигнала по задержке: 3 — до 120 мс, 2 — до 250, 1 — дольше, 0 — не мерили; null — не отвечает. */
function barsOf(server: VpnServer): number | null {
    if (server.up === false) return null;
    if (typeof server.delayMs !== 'number') return 0;
    return server.delayMs <= 120 ? 3 : server.delayMs <= 250 ? 2 : 1;
}

function readRecent(): Recent[] {
    try {
        const raw = JSON.parse(localStorage.getItem(RECENT_KEY) ?? '[]') as unknown;
        return Array.isArray(raw) ? (raw as Recent[]).filter((one) => one && typeof one.name === 'string').slice(0, RECENT_MAX) : [];
    } catch {
        return [];
    }
}

function rememberRecent(entry: Recent): Recent[] {
    const next = [entry, ...readRecent().filter((one) => !(one.connection === entry.connection && one.name === entry.name))].slice(0, RECENT_MAX);
    try {
        localStorage.setItem(RECENT_KEY, JSON.stringify(next));
    } catch {
        // без памяти браузера — без «Недавних»
    }
    return next;
}

/** Слова программы из отказа: шлюз заворачивает их в «…{"error":"…"}». */
function errorText(error: unknown): string {
    const message = problemOf(error);
    const found = message.match(/"error"\s*:\s*"((?:[^"\\]|\\.)*)"/);
    if (!found) return message;
    try {
        return JSON.parse(`"${found[1]}"`) as string;
    } catch {
        return found[1];
    }
}

function names(list: string[]): string {
    const shown = list.slice(0, NAMES_SHOWN);
    if (list.length > NAMES_SHOWN) shown.push(tr.egress.andMore(list.length - NAMES_SHOWN));
    return formatList(shown);
}

function connectionLabel(connection: EgressConnection | VpnConnection | undefined, id: string): string {
    if (!connection) return id;
    return `${flagOf(connection.country ?? null)} ${connection.name}`.trim();
}

/** Строка «отмечена» в списке «Режима VPN для DuoWave». */
function isChosen(view: EgressView, row: EgressRow): boolean {
    if (view.pick === 'except') return row.route === 'direct';
    if (view.pick === 'only') return row.route !== 'auto' && row.route !== 'direct';
    return false;
}

// ─── Значки ─────────────────────────────────────────────────────────────────────────

const ICON = { width: 18, height: 18, viewBox: '0 0 24 24', fill: 'none', stroke: 'currentColor', 'stroke-width': 2, 'stroke-linecap': 'round', 'stroke-linejoin': 'round' } as const;
const IconPower = () => (
    <svg {...ICON} aria-hidden="true">
        <path d="M12 3v8" />
        <path d="M6.3 7.3a8 8 0 1 0 11.4 0" />
    </svg>
);
const IconChevron = () => (
    <svg {...ICON} width={16} height={16} aria-hidden="true">
        <path d="m9 6 6 6-6 6" />
    </svg>
);
const IconBack = () => (
    <svg {...ICON} width={16} height={16} aria-hidden="true">
        <path d="m15 6-6 6 6 6" />
    </svg>
);
const IconSpark = () => (
    <svg {...ICON} aria-hidden="true">
        <path d="M12 3l1.8 5.2L19 10l-5.2 1.8L12 17l-1.8-5.2L5 10l5.2-1.8z" />
        <path d="M19 15l.7 2 2 .7-2 .7-.7 2-.7-2-2-.7 2-.7z" />
    </svg>
);
const IconShield = () => (
    <svg {...ICON} width={16} height={16} aria-hidden="true">
        <path d="M12 3l7 3v5c0 4.5-3 8-7 10-4-2-7-5.5-7-10V6z" />
    </svg>
);
const IconCheck = () => (
    <svg {...ICON} width={16} height={16} aria-hidden="true">
        <path d="M20 6 9 17l-5-5" />
    </svg>
);
const IconSort = () => (
    <svg {...ICON} aria-hidden="true">
        <path d="M7 4v16M3 16l4 4 4-4M17 20V4M13 8l4-4 4 4" />
    </svg>
);

function Bars({ level }: { level: number | null }) {
    return (
        <span class={`vpn-bars${level === null ? ' vpn-bars-dead' : ''}`} aria-hidden="true">
            {[1, 2, 3].map((step) => (
                <i key={step} class={level !== null && level >= step ? 'on' : ''} style={{ height: `${4 + step * 4}px` }} />
            ))}
        </span>
    );
}

function Flag({ code }: { code: string | null }) {
    return (
        <span class="vpn-flag" aria-hidden="true">
            {code ? flagOf(code) : <IconShield />}
        </span>
    );
}

/** Строка-кнопка списка: значок, две строки текста, что-то справа, шеврон. */
function LineButton({ icon, title, sub, aside, chevron = true, onClick, disabled }: { icon?: ComponentChildren; title: ComponentChildren; sub?: ComponentChildren; aside?: ComponentChildren; chevron?: boolean; onClick: () => void; disabled?: boolean }) {
    return (
        <button type="button" class="vpn-line" onClick={onClick} disabled={disabled}>
            {icon}
            <span class="vpn-line-text">
                <span class="vpn-line-title">{title}</span>
                {sub && <span class="vpn-line-sub">{sub}</span>}
            </span>
            {aside}
            {chevron && (
                <span class="vpn-chevron">
                    <IconChevron />
                </span>
            )}
        </button>
    );
}

function Group({ title, children }: { title?: string; children: ComponentChildren }) {
    return (
        <section class="vpn-group">
            {title && <h3 class="vpn-group-title">{title}</h3>}
            <div class="card glass vpn-card">{children}</div>
        </section>
    );
}

// ─── Состояние экрана ──────────────────────────────────────────────────────────────

interface Vpn {
    view: EgressView;
    status: VpnStatus | null;
    statusProblem: string | null;
    busy: boolean;
    change: (work: () => Promise<EgressView | void>) => Promise<void>;
    call: <T = unknown>(fn: string, input?: unknown) => Promise<T>;
    reload: () => void;
    mayEdit: boolean;
    /**
     * Приложение VPN стоит, но выключено (его выключил надзор или человек): подключения и подписки на месте, только
     * программа не идёт. Окно говорит «VPN выключен» и включает его, а не «Нет подписки» (проверка начисто 18.11, п. 18).
     */
    appOff: boolean;
}

function useVpn(): { vpn: Vpn | null; problem: string | null; setProblem: (text: string | null) => void } {
    const [view, setView] = useState<EgressView | null>(null);
    const [status, setStatus] = useState<VpnStatus | null>(null);
    const [statusProblem, setStatusProblem] = useState<string | null>(null);
    const [problem, setProblem] = useState<string | null>(null);
    const [busy, setBusy] = useState(false);
    const [appOff, setAppOff] = useState(false);
    const [tick, setTick] = useState(0);
    const reload = useCallback(() => setTick((now) => now + 1), []);
    useEffect(() => {
        let alive = true;
        api.egress().then(
            (answer) => {
                if (!alive) return;
                setView(answer);
                if (answer.app)
                    api.callPackage<VpnStatus>(answer.app, 'status').then(
                        (next) => {
                            if (!alive) return;
                            setStatus(next);
                            setStatusProblem(null);
                            setAppOff(false);
                        },
                        (error) => {
                            if (!alive) return;
                            setStatusProblem(errorText(error));
                            // Не ответило — выключено ли оно: тогда это не «нет подписки», а «VPN выключен».
                            api.packages().then(
                                (list) => alive && setAppOff(list.packages.find((entry) => entry.id === answer.app)?.enabled === false),
                                () => undefined,
                            );
                        },
                    );
                else setStatus(null);
            },
            (error) => alive && setProblem(problemOf(error)),
        );
        return () => {
            alive = false;
        };
    }, [tick]);
    const starting = (status?.connections ?? []).some((one) => one.state === 'starting' || one.subscription?.reading);
    useEffect(() => {
        const timer = window.setInterval(reload, starting ? BUSY_MS : REFRESH_MS);
        return () => window.clearInterval(timer);
    }, [reload, starting]);
    const change = useCallback(
        async (work: () => Promise<EgressView | void>) => {
            setBusy(true);
            setProblem(null);
            try {
                const answer = await work();
                if (answer) setView(answer);
            } catch (error) {
                setProblem(errorText(error));
            } finally {
                setBusy(false);
                reload();
            }
        },
        [reload],
    );
    const app = view?.app ?? null;
    const call = useCallback(<T,>(fn: string, input: unknown = {}) => api.callPackage<T>(app ?? 'own-server', fn, input), [app]);
    if (!view) return { vpn: null, problem, setProblem };
    return { vpn: { view, status, statusProblem, busy, change, call, reload, mayEdit: can('settings.edit'), appOff }, problem, setProblem };
}

/** Подключение, через которое идут «все»: выбранное, иначе первое, что отвечает. */
function mainConnection(vpn: Vpn): VpnConnection | undefined {
    const list = vpn.status?.connections ?? [];
    return list.find((one) => one.id === vpn.view.preferred) ?? list.find((one) => one.state === 'up') ?? list[0];
}

// ─── Главная ─────────────────────────────────────────────────────────────────────

function HomeTab({ vpn, go }: { vpn: Vpn; go: (tab: Tab) => void }) {
    const locale = useLanguage();
    const panel = usePanel();
    const { view } = vpn;
    const connections = vpn.status?.connections ?? [];
    const main = mainConnection(vpn);
    const [recent, setRecent] = useState<Recent[]>(readRecent);
    const down = view.on && view.rows.some((row) => row.decision.kind === 'down');
    const through = view.rows.filter((row) => row.decision.kind === 'via' || row.decision.kind === 'down');
    const state = !view.anyReported && connections.length === 0 ? 'none' : !view.on ? 'off' : down ? 'down' : 'on';
    const hint =
        state === 'none'
            ? tr.egress.hintNone
            : state === 'off'
              ? tr.egress.hintOff
              : state === 'down'
                ? view.onDownAll === 'wait'
                    ? tr.egress.hintDownWait
                    : view.onDownAll === 'direct'
                      ? tr.egress.hintDownDirect
                      : tr.egress.hintDownSafe
                : through.length
                  ? tr.egress.hintVia(names(through.map((row) => row.name)))
                  : tr.egress.hintNothing;
    const current = main?.current ? main.servers.find((one) => one.name === main.current) ?? { name: main.current } : undefined;
    const place = current ? placeOf(current, main, locale) : null;
    const auto = !main || main.mode === 'auto';
    const chosenCount = view.rows.filter((row) => isChosen(view, row)).length;
    const home = view.home && view.home !== 'none' ? countryName(view.home, locale) : null;
    const pickServer = (entry: Recent) =>
        vpn.change(async () => {
            await vpn.call('select', { connection: entry.connection, name: entry.name });
            setRecent(rememberRecent(entry));
            if (connections.length > 1 && view.preferred !== entry.connection) return api.setEgress({ preferred: entry.connection });
        });
    const pickAuto = () =>
        vpn.change(async () => {
            for (const one of connections) if (one.mode !== 'auto') await vpn.call('select', { connection: one.id, name: 'auto' });
            if (view.preferred) return api.setEgress({ preferred: null });
        });
    return (
        <div class="vpn-tab">
            {!view.app && (
                <div class="vpn-notice">
                    <p class="m-0">{tr.egress.noApp}</p>
                    <Action label={tr.egress.openMagicPlace} onClick={() => panel.go('magicplace', 'store', 'own-server')} />
                </div>
            )}
            {view.app && !view.anyReported && connections.length > 0 && (
                <div class="vpn-notice">
                    <p class="m-0">{tr.egress.noYes}</p>
                    <Action label={tr.egress.openLinks} onClick={() => panel.go('skills', 'installed', view.app ?? undefined)} />
                </div>
            )}
            {view.app && connections.length > 0 && !view.home && view.homeDirect && view.pick !== 'only' && (
                <div class="vpn-notice">
                    <p class="m-0">
                        <b>{tr.egress.homeAsk}</b> {tr.egress.homeAskAbout}
                    </p>
                    <Action label={tr.egress.homeAskAction} tone="on" onClick={() => go('settings')} />
                </div>
            )}
            <section class={`card glass vpn-hero vpn-hero-${state}`}>
                <p class="vpn-hint">
                    <IconShield />
                    <span>{hint}</span>
                </p>
                {connections.length === 0 ? (
                    <LineButton icon={<Flag code={null} />} title={tr.egress.placeNone} sub={tr.egress.placeNoneSub} onClick={() => go('subs')} disabled={!view.app} />
                ) : (
                    <LineButton
                        icon={<Flag code={place?.code ?? null} />}
                        title={
                            <>
                                <span class="vpn-line-label">{state === 'on' ? tr.egress.placeConnected : tr.egress.placeChosen}</span>
                                {auto ? (
                                    <span>
                                        {tr.egress.placeAuto}
                                        {place && <span class="vpn-city"> · {tr.egress.placeAutoNow(place.city ? `${place.country} — ${place.city}` : place.country)}</span>}
                                    </span>
                                ) : (
                                    <span>
                                        {place?.country}
                                        {place?.city && <span class="vpn-city"> — {place.city}</span>}
                                    </span>
                                )}
                            </>
                        }
                        onClick={() => go('places')}
                    />
                )}
                <div class="vpn-state">
                    <p class="vpn-state-title">{state === 'none' ? tr.egress.stateNone : state === 'off' ? tr.egress.stateOff : state === 'down' ? tr.egress.stateDown : tr.egress.stateOn}</p>
                    <div class="vpn-power" role="radiogroup" aria-label={tr.egress.switchLabel} aria-busy={vpn.busy}>
                        <button type="button" role="radio" aria-checked={!view.on} disabled={!vpn.mayEdit || vpn.busy} onClick={() => view.on && void vpn.change(() => api.setEgress({ on: false }))}>
                            {!view.on && <IconPower />}
                            {tr.egress.switchOff}
                        </button>
                        <button
                            type="button"
                            role="radio"
                            aria-checked={view.on}
                            disabled={!vpn.mayEdit || vpn.busy || (!view.on && connections.length === 0)}
                            onClick={() => !view.on && void vpn.change(() => api.setEgress({ on: true }))}
                        >
                            {view.on && <IconPower />}
                            {tr.egress.switchOn}
                        </button>
                    </div>
                    {view.on && (
                        <button type="button" class="vpn-link" onClick={() => go('settings')}>
                            {view.pick === 'only'
                                ? tr.egress.onlyLine(chosenCount)
                                : view.pick === 'except'
                                  ? tr.egress.exceptLine(chosenCount)
                                  : view.homeDirect
                                    ? home && view.home
                                        ? tr.egress.homeDirectLine(`${flagOf(view.home)} ${home}`)
                                        : tr.egress.homeDirectLineNoCountry
                                    : tr.egress.pickAllSub}
                            <IconChevron />
                        </button>
                    )}
                </div>
            </section>
            {connections.length > 0 && !auto && (
                <section class="card glass vpn-card">
                    <LineButton
                        icon={
                            <span class="vpn-spark">
                                <IconSpark />
                            </span>
                        }
                        title={
                            <span class="vpn-accent">
                                {tr.egress.autoTitle} <span class="vpn-badge">{tr.egress.autoBadge}</span>
                            </span>
                        }
                        sub={tr.egress.autoSub}
                        chevron={false}
                        disabled={!vpn.mayEdit || vpn.busy}
                        onClick={() => void pickAuto()}
                    />
                </section>
            )}
            {recent.length > 0 && connections.length > 0 && (
                <Group title={tr.egress.recent}>
                    {recent.map((entry) => {
                        const connection = connections.find((one) => one.id === entry.connection);
                        const server = connection?.servers.find((one) => one.name === entry.name);
                        if (!connection || !server) return null;
                        const where = placeOf(server, connection, locale);
                        return (
                            <LineButton
                                key={`${entry.connection}/${entry.name}`}
                                icon={<Flag code={where.code} />}
                                title={
                                    <span>
                                        {where.country}
                                        {where.city && <span class="vpn-city"> — {where.city}</span>}
                                    </span>
                                }
                                aside={<Bars level={barsOf(server)} />}
                                chevron={false}
                                disabled={!vpn.mayEdit || vpn.busy}
                                onClick={() => void pickServer(entry)}
                            />
                        );
                    })}
                </Group>
            )}
        </div>
    );
}

// ─── Локации ─────────────────────────────────────────────────────────────────────

function PlacesTab({ vpn }: { vpn: Vpn }) {
    const locale = useLanguage();
    const [query, setQuery] = useState('');
    const [sort, setSort] = useState<'speed' | 'name'>('speed');
    const connections = vpn.status?.connections ?? [];
    const main = mainConnection(vpn);
    const auto = !main || main.mode === 'auto';
    const many = connections.length > 1;
    const all = useMemo(
        () =>
            connections.flatMap((connection) =>
                connection.servers.map((server) => ({ connection, server, place: placeOf(server, connection, locale) })),
            ),
        [connections, locale],
    );
    const needle = query.trim().toLocaleLowerCase(locale);
    const shown = all
        .filter((one) => !needle || `${one.place.country} ${one.place.city} ${one.server.name} ${one.connection.name}`.toLocaleLowerCase(locale).includes(needle))
        .sort((a, b) => {
            if (sort === 'name') return `${a.place.country} ${a.place.city}`.localeCompare(`${b.place.country} ${b.place.city}`, locale);
            const delay = (one: VpnServer) => (one.up === false ? 1e9 : typeof one.delayMs === 'number' ? one.delayMs : 1e8);
            return delay(a.server) - delay(b.server);
        });
    const best = all.filter((one) => one.server.up !== false && typeof one.server.delayMs === 'number').sort((a, b) => (a.server.delayMs ?? 0) - (b.server.delayMs ?? 0))[0];
    const pick = (connection: VpnConnection, server: VpnServer) =>
        vpn.change(async () => {
            await vpn.call('select', { connection: connection.id, name: server.name });
            rememberRecent({ connection: connection.id, name: server.name });
            if (many && vpn.view.preferred !== connection.id) return api.setEgress({ preferred: connection.id });
        });
    const pickAuto = () =>
        vpn.change(async () => {
            for (const one of connections) if (one.mode !== 'auto') await vpn.call('select', { connection: one.id, name: 'auto' });
            if (vpn.view.preferred) return api.setEgress({ preferred: null });
        });
    if (connections.length === 0) return <p class="vpn-empty">{tr.egress.placesEmpty}</p>;
    return (
        <div class="vpn-tab">
            <div class="vpn-search">
                <input class="field vpn-search-field" type="search" placeholder={tr.egress.search} aria-label={tr.egress.search} value={query} onInput={(event) => setQuery((event.target as HTMLInputElement).value)} />
                <button type="button" class="vpn-icon-btn" title={sort === 'speed' ? tr.egress.sortName : tr.egress.sortSpeed} aria-label={sort === 'speed' ? tr.egress.sortName : tr.egress.sortSpeed} onClick={() => setSort(sort === 'speed' ? 'name' : 'speed')}>
                    <IconSort />
                </button>
            </div>
            <section class="card glass vpn-card">
                {!needle && (
                    <LineButton
                        icon={
                            <span class="vpn-spark vpn-spark-filled">
                                <IconSpark />
                            </span>
                        }
                        title={
                            <span>
                                {tr.egress.autoTitle} <span class="vpn-badge">{tr.egress.autoBadge}</span>
                            </span>
                        }
                        sub={tr.egress.autoSub}
                        aside={
                            <>
                                {auto && (
                                    <span class="vpn-check">
                                        <IconCheck />
                                    </span>
                                )}
                                <Bars level={best ? barsOf(best.server) : 0} />
                            </>
                        }
                        chevron={false}
                        disabled={!vpn.mayEdit || vpn.busy}
                        onClick={() => void pickAuto()}
                    />
                )}
                {shown.length === 0 && <p class="vpn-empty">{tr.egress.nothingFound}</p>}
                {shown.map(({ connection, server, place }) => {
                    const chosen = !auto && connection.id === main?.id && connection.mode === 'manual' && connection.choice === server.name;
                    const dead = server.up === false;
                    return (
                        <LineButton
                            key={`${connection.id}/${server.name}`}
                            icon={<Flag code={place.code} />}
                            title={
                                <span class={chosen ? 'vpn-accent' : dead ? 'vpn-dead' : ''}>
                                    {place.country}
                                    {place.city && <span class="vpn-city"> — {place.city}</span>}
                                </span>
                            }
                            sub={[many ? connection.name : null, dead ? tr.egress.deadServer : typeof server.delayMs === 'number' ? tr.egress.ms(server.delayMs) : null].filter(Boolean).join(' · ') || undefined}
                            aside={
                                <>
                                    {chosen && (
                                        <span class="vpn-check">
                                            <IconCheck />
                                        </span>
                                    )}
                                    <Bars level={barsOf(server)} />
                                </>
                            }
                            chevron={false}
                            disabled={!vpn.mayEdit || vpn.busy}
                            onClick={() => void pick(connection, server)}
                        />
                    );
                })}
            </section>
        </div>
    );
}

// ─── Настройки ───────────────────────────────────────────────────────────────────

const PICKS: Array<{ id: EgressPick; title: () => string; sub: () => string; cells: boolean[] }> = [
    { id: 'all', title: () => tr.egress.pickAll, sub: () => tr.egress.pickAllSub, cells: [true, true, true, true] },
    { id: 'except', title: () => tr.egress.pickExcept, sub: () => tr.egress.pickExceptSub, cells: [false, true, true, true] },
    { id: 'only', title: () => tr.egress.pickOnly, sub: () => tr.egress.pickOnlySub, cells: [true, true, false, false] },
];

/** Значок режима: четыре клетки — залитые идут через VPN, с галочкой — выбранные. */
function PickIcon({ id }: { id: EgressPick }) {
    const cell = (index: number) => {
        if (id === 'all') return 'fill';
        if (id === 'except') return index === 0 ? 'tick' : 'fill';
        return index < 2 ? 'tick' : 'empty';
    };
    return (
        <span class="vpn-pick-icon" aria-hidden="true">
            {[0, 1, 2, 3].map((index) => (
                <i key={index} class={cell(index)}>
                    {cell(index) === 'tick' && <IconCheck />}
                </i>
            ))}
        </span>
    );
}

function PickPage({ vpn, onBack }: { vpn: Vpn; onBack: () => void }) {
    const locale = useLanguage();
    const { view } = vpn;
    const [query, setQuery] = useState('');
    const many = view.connections.length > 1;
    const rows = view.rows;
    const chosen = rows.filter((row) => isChosen(view, row));
    const needle = query.trim().toLocaleLowerCase(locale);
    const shown = rows.filter((row) => !needle || `${row.name} ${tr.egress.sections[row.section] ?? ''} ${row.hosts.join(' ')}`.toLocaleLowerCase(locale).includes(needle));
    const listOff = view.pick === 'all';
    const tick = (row: EgressRow) => {
        const on = isChosen(view, row);
        const route = on ? 'auto' : view.pick === 'except' ? 'direct' : 'server';
        void vpn.change(() => api.setEgressRow(row.id, { route }));
    };
    return (
        <div class="vpn-tab">
            <button type="button" class="vpn-back" onClick={onBack}>
                <IconBack />
                {tr.egress.back}
            </button>
            <h3 class="vpn-page-title">{tr.egress.pickTitle}</h3>
            <section class="card glass vpn-card" role="radiogroup" aria-label={tr.egress.pickTitle}>
                {PICKS.map((pick) => (
                    <button
                        key={pick.id}
                        type="button"
                        role="radio"
                        aria-checked={view.pick === pick.id}
                        class="vpn-line vpn-radio-line"
                        disabled={!vpn.mayEdit || vpn.busy}
                        onClick={() => view.pick !== pick.id && void vpn.change(() => api.setEgress({ pick: pick.id }))}
                    >
                        <PickIcon id={pick.id} />
                        <span class="vpn-line-text">
                            <span class="vpn-line-title">{pick.title()}</span>
                            <span class="vpn-line-sub">{pick.sub()}</span>
                        </span>
                        <span class="vpn-radio" aria-hidden="true" />
                    </button>
                ))}
            </section>
            <section class={`vpn-group${listOff ? ' vpn-off' : ''}`} aria-disabled={listOff}>
                <h3 class="vpn-group-title">{tr.egress.chosenHead(listOff ? 0 : chosen.length, rows.length)}</h3>
                <div class="card glass vpn-card">
                    <div class="vpn-card-pad">
                        <input class="field vpn-search-field" type="search" placeholder={tr.egress.search} aria-label={tr.egress.search} value={query} disabled={listOff} onInput={(event) => setQuery((event.target as HTMLInputElement).value)} />
                    </div>
                    {shown.map((row) => {
                        const on = !listOff && isChosen(view, row);
                        const goesVia = view.pick === 'only' && on;
                        return (
                            <div key={row.id} class="vpn-check-line">
                                <label class="vpn-check-label">
                                    <span class="vpn-line-text">
                                        <span class="vpn-line-title">
                                            {row.name}
                                            {row.new && <span class="vpn-badge vpn-badge-new">{tr.egress.isNew}</span>}
                                        </span>
                                        {tr.egress.sections[row.section] !== row.name && <span class="vpn-line-sub">{tr.egress.sections[row.section]}</span>}
                                    </span>
                                    <input type="checkbox" class="vpn-checkbox" checked={on} disabled={listOff || !vpn.mayEdit || vpn.busy} onChange={() => tick(row)} />
                                </label>
                                {goesVia && many && (
                                    <select
                                        class="vpn-select"
                                        value={row.route === 'server' ? 'server' : row.route}
                                        disabled={!vpn.mayEdit || vpn.busy}
                                        aria-label={row.name}
                                        onChange={(event) => void vpn.change(() => api.setEgressRow(row.id, { route: (event.target as HTMLSelectElement).value }))}
                                    >
                                        <option value="server">{tr.egress.viaChosen}</option>
                                        {view.connections.map((connection) => (
                                            <option key={connection.id} value={connection.id}>
                                                {tr.egress.viaConnection(connectionLabel(connection, connection.id))}
                                            </option>
                                        ))}
                                    </select>
                                )}
                            </div>
                        );
                    })}
                </div>
            </section>
        </div>
    );
}

function HomeCountry({ vpn }: { vpn: Vpn }) {
    const locale = useLanguage();
    const [picking, setPicking] = useState(false);
    const [query, setQuery] = useState('');
    const may = can('plugins.manage') && !!vpn.view.app;
    const all = useMemo(() => COUNTRIES.map((code) => ({ code, name: countryName(code, locale) })).sort((a, b) => a.name.localeCompare(b.name, locale)), [locale]);
    const needle = query.trim().toLocaleLowerCase(locale);
    const shown = needle ? all.filter((one) => one.name.toLocaleLowerCase(locale).includes(needle) || one.code.toLowerCase() === needle) : all;
    const home = vpn.view.home;
    const save = (code: string) =>
        vpn.change(async () => {
            await api.setPackageSettings(vpn.view.app ?? 'own-server', { homeCountry: code });
            setPicking(false);
            setQuery('');
        });
    return (
        <>
            <div class="vpn-set-row">
                <span class="vpn-line-text">
                    <span class="vpn-line-title">{home && home !== 'none' ? `${flagOf(home)} ${countryName(home, locale)}` : home === 'none' ? tr.egress.homeWontSay : tr.egress.homeNone}</span>
                    <span class="vpn-line-sub">{tr.egress.homeAbout}</span>
                </span>
                {may && <Action label={picking ? tr.egress.homeCancel : tr.egress.homeChange} onClick={() => setPicking(!picking)} />}
            </div>
            {picking && (
                <div class="vpn-card-pad vpn-countries-wrap">
                    <input class="field vpn-search-field" type="search" placeholder={tr.egress.homeSearch} aria-label={tr.egress.homeSearch} value={query} onInput={(event) => setQuery((event.target as HTMLInputElement).value)} />
                    <div class="vpn-countries" role="listbox" aria-label={tr.egress.homeGroup}>
                        {!needle && (
                            <button type="button" role="option" class="vpn-country" onClick={() => void save('none')}>
                                <span class="vpn-country-flag" />
                                {tr.egress.homeWontSay}
                            </button>
                        )}
                        {shown.length === 0 && <p class="vpn-empty">{tr.egress.homeNothing}</p>}
                        {shown.map((one) => (
                            <button key={one.code} type="button" role="option" aria-selected={home === one.code} class="vpn-country" onClick={() => void save(one.code)}>
                                <span class="vpn-country-flag">{flagOf(one.code)}</span>
                                {one.name}
                            </button>
                        ))}
                    </div>
                </div>
            )}
        </>
    );
}

function CustomHosts({ vpn }: { vpn: Vpn }) {
    const [text, setText] = useState('');
    const { view } = vpn;
    const add = (): void => {
        const host = text.trim();
        if (!host) return;
        void vpn.change(() => api.setEgressCustom([...view.custom, host]));
        setText('');
    };
    return (
        <div class="vpn-card-pad vpn-custom">
            <p class="vpn-note">{tr.egress.customAbout}</p>
            {vpn.mayEdit && (
                <div class="vpn-custom-add">
                    <input class="field vpn-search-field" placeholder={tr.egress.customLabel} aria-label={tr.egress.customLabel} value={text} onInput={(event) => setText((event.target as HTMLInputElement).value)} onKeyDown={(event) => event.key === 'Enter' && add()} />
                    <Action label={tr.egress.customAdd} disabled={!text.trim()} onClick={add} />
                </div>
            )}
            {view.custom.length === 0 && <p class="vpn-note">{tr.egress.customEmpty}</p>}
            {view.custom.map((host) => (
                <div key={host} class="vpn-host">
                    <span>{host}</span>
                    {vpn.mayEdit && (
                        <button type="button" class="vpn-text-btn" onClick={() => void vpn.change(() => api.setEgressCustom(view.custom.filter((one) => one !== host)))}>
                            {tr.egress.customRemove}
                        </button>
                    )}
                </div>
            ))}
        </div>
    );
}

const ON_DOWN: Array<{ id: EgressOnDownAll; label: () => string; about: () => string }> = [
    { id: 'safe', label: () => tr.egress.onDownSafe, about: () => tr.egress.onDownSafeAbout },
    { id: 'wait', label: () => tr.egress.onDownWait, about: () => tr.egress.onDownWaitAbout },
    { id: 'direct', label: () => tr.egress.onDownDirect, about: () => tr.egress.onDownDirectAbout },
];

function SettingsTab({ vpn }: { vpn: Vpn }) {
    const locale = useLanguage();
    const [page, setPage] = useState<'main' | 'pick'>('main');
    const { view } = vpn;
    if (page === 'pick') return <PickPage vpn={vpn} onBack={() => setPage('main')} />;
    const home = view.home && view.home !== 'none' ? `${flagOf(view.home)} ${countryName(view.home, locale)}` : null;
    const chosen = view.rows.filter((row) => isChosen(view, row)).length;
    const pickName = view.pick === 'all' ? tr.egress.pickAll : view.pick === 'except' ? tr.egress.pickExcept : tr.egress.pickOnly;
    const onDown = ON_DOWN.find((one) => one.id === view.onDownAll) ?? ON_DOWN[0];
    return (
        <div class="vpn-tab">
            <Group title={tr.egress.whoGroup}>
                <div class={`vpn-set-row${view.pick === 'only' ? ' vpn-off' : ''}`}>
                    <span class="vpn-line-text">
                        <span class="vpn-line-title">{tr.egress.homeDirect}</span>
                        <span class="vpn-line-sub">{view.pick === 'only' ? tr.egress.homeDirectOnly : home ? tr.egress.homeDirectSub(home) : tr.egress.homeDirectNoCountry}</span>
                    </span>
                    <Switch on={view.homeDirect} label={tr.egress.homeDirect} disabled={!vpn.mayEdit || vpn.busy || view.pick === 'only'} onChange={(on) => void vpn.change(() => api.setEgress({ homeDirect: on }))} />
                </div>
                <LineButton
                    title={tr.egress.pickTitle}
                    sub={<span class="vpn-accent">{pickName}</span>}
                    aside={view.pick !== 'all' ? <span class="vpn-count">{chosen}</span> : undefined}
                    onClick={() => setPage('pick')}
                />
            </Group>
            <Group title={tr.egress.onDownGroup}>
                <div class="vpn-card-pad vpn-ondown">
                    <select class="field vpn-select-wide" value={view.onDownAll} disabled={!vpn.mayEdit || vpn.busy} aria-label={tr.egress.onDownGroup} onChange={(event) => void vpn.change(() => api.setEgress({ onDownAll: (event.target as HTMLSelectElement).value as EgressOnDownAll }))}>
                        {ON_DOWN.map((one) => (
                            <option key={one.id} value={one.id}>
                                {one.label()}
                            </option>
                        ))}
                    </select>
                    <p class={`vpn-note${onDown.id === 'direct' ? ' vpn-warn' : ''}`}>{onDown.about()}</p>
                </div>
            </Group>
            <Group title={tr.egress.homeGroup}>
                <HomeCountry vpn={vpn} />
            </Group>
            <Group title={tr.egress.ownGroup}>
                <CustomHosts vpn={vpn} />
            </Group>
            <p class="vpn-note vpn-foot">{tr.egress.always}</p>
        </div>
    );
}

// ─── Подписки ────────────────────────────────────────────────────────────────────

function AddForm({ vpn, onClose }: { vpn: Vpn; onClose: () => void }) {
    const [name, setName] = useState('');
    const [source, setSource] = useState('');
    const [busy, setBusy] = useState(false);
    const [error, setError] = useState<string | null>(null);
    const save = async () => {
        setBusy(true);
        setError(null);
        try {
            await vpn.call('add', { name: name.trim(), source });
            onClose();
            vpn.reload();
        } catch (problem) {
            setError(errorText(problem));
        } finally {
            setBusy(false);
        }
    };
    return (
        <Group title={tr.egress.addTitle}>
            <div class="vpn-card-pad vpn-form">
                <label class="vpn-field">
                    <span class="vpn-label">{tr.egress.name}</span>
                    <input class="field" type="text" value={name} placeholder={tr.egress.namePlaceholder} maxLength={60} onInput={(event) => setName((event.target as HTMLInputElement).value)} />
                </label>
                <label class="vpn-field">
                    <span class="vpn-label">{tr.egress.source}</span>
                    <textarea class="field vpn-source" rows={5} value={source} spellcheck={false} autocomplete="off" placeholder={tr.egress.sourcePlaceholder} onInput={(event) => setSource((event.target as HTMLTextAreaElement).value)} />
                </label>
                <div class="vpn-help">
                    <p class="vpn-note">{tr.egress.addWhat}</p>
                    <ul>
                        <li>{tr.egress.addSub}</li>
                        <li>{tr.egress.addLinks}</li>
                        <li>{tr.egress.addWg}</li>
                    </ul>
                    <p class="vpn-note">{tr.egress.addPrivate}</p>
                </div>
                {error && (
                    <p class="vpn-note c-dot-offline" role="alert">
                        {error}
                    </p>
                )}
                <div class="vpn-actions">
                    <Action label={tr.egress.save} tone="on" busy={busy} disabled={!source.trim()} onClick={() => void save()} />
                    <Action label={tr.egress.cancel} disabled={busy} onClick={onClose} />
                </div>
            </div>
        </Group>
    );
}

function ConnectionCard({ vpn, connection }: { vpn: Vpn; connection: VpnConnection }) {
    const locale = useLanguage();
    const [note, setNote] = useState<{ error: boolean; text: string } | null>(null);
    const [busy, setBusy] = useState<string | null>(null);
    const [renaming, setRenaming] = useState(false);
    const [newName, setNewName] = useState(connection.name);
    const run = async (what: string, work: () => Promise<void>) => {
        setBusy(what);
        setNote(null);
        try {
            await work();
        } catch (error) {
            setNote({ error: true, text: tr.egress.failed(errorText(error)) });
        } finally {
            setBusy(null);
            vpn.reload();
        }
    };
    const sub = connection.subscription;
    const when = sub?.updatedAt ? new Intl.DateTimeFormat(locale, { dateStyle: 'short', timeStyle: 'short' }).format(new Date(sub.updatedAt * 1000)) : null;
    const reading = !!sub?.reading && connection.servers.length === 0;
    const state = reading ? 'starting' : connection.state;
    const stateText = reading ? tr.egress.connReading : state === 'up' ? tr.egress.connUp : state === 'down' ? tr.egress.connDown : tr.egress.connStarting;
    const meta = [tr.egress.servers(connection.servers.length), sub ? tr.egress.subHost(sub.host) : null, when ? tr.egress.updated(when) : null].filter(Boolean).join(' · ');
    return (
        <section class="card glass vpn-card vpn-conn">
            <div class="vpn-conn-head">
                {renaming ? (
                    <div class="vpn-rename">
                        <input
                            class="field"
                            type="text"
                            value={newName}
                            maxLength={60}
                            aria-label={tr.egress.rename}
                            onInput={(event) => setNewName((event.target as HTMLInputElement).value)}
                            onKeyDown={(event) => {
                                if (event.key === 'Enter' && newName.trim()) void run('rename', () => vpn.call('rename', { connection: connection.id, name: newName.trim() }).then(() => setRenaming(false)));
                                if (event.key === 'Escape') setRenaming(false);
                            }}
                        />
                        <Action label={tr.egress.renameSave} tone="on" busy={busy === 'rename'} disabled={!newName.trim()} onClick={() => void run('rename', () => vpn.call('rename', { connection: connection.id, name: newName.trim() }).then(() => setRenaming(false)))} />
                        <Action label={tr.egress.cancel} onClick={() => setRenaming(false)} />
                    </div>
                ) : (
                    <>
                        <span class="vpn-line-text">
                            <span class="vpn-conn-name">{connectionLabel(connection, connection.id)}</span>
                            <span class="vpn-line-sub">{meta}</span>
                        </span>
                        <span class={`vpn-status vpn-status-${state}`}>
                            <i />
                            {stateText}
                        </span>
                    </>
                )}
            </div>
            {sub?.error && <p class="vpn-note vpn-warn">{`${tr.egress.subHost(sub.host)} — ${sub.error}`}</p>}
            {connection.skipped.length > 0 && (
                <p class="vpn-note vpn-warn">
                    {tr.egress.skipped(connection.skipped.length)} {connection.skipped.slice(0, 5).map((one) => `${one.scheme} — ${one.reason}`).join('; ')}
                </p>
            )}
            {note && <p class={`vpn-note${note.error ? ' c-dot-offline' : ''}`}>{note.text}</p>}
            {vpn.mayEdit && (
                <div class="vpn-actions vpn-actions-split">
                    <div class="vpn-actions">
                        <Action
                            label={tr.egress.check}
                            busy={busy === 'check'}
                            disabled={busy !== null || connection.servers.length === 0}
                            onClick={() =>
                                void run('check', async () => {
                                    const answer = await vpn.call<{ pending?: boolean; country?: string; delayMs?: number }>('check', { connection: connection.id });
                                    if (answer?.pending) setNote({ error: false, text: tr.egress.checkPending });
                                    else if (answer?.country) setNote({ error: false, text: tr.egress.checked(`${flagOf(answer.country)} ${countryName(answer.country, locale)}`, answer.delayMs ?? 0) });
                                })
                            }
                        />
                        {sub && <Action label={tr.egress.refresh} busy={busy === 'refresh' || !!sub.reading} disabled={busy !== null} onClick={() => void run('refresh', () => vpn.call('refresh', { connection: connection.id }))} />}
                        {!renaming && (
                            <Action
                                label={tr.egress.rename}
                                disabled={busy !== null}
                                onClick={() => {
                                    setNewName(connection.name);
                                    setRenaming(true);
                                }}
                            />
                        )}
                    </div>
                    <button
                        type="button"
                        class="vpn-text-btn vpn-danger"
                        disabled={busy !== null}
                        onClick={() =>
                            void ask({ title: tr.egress.removeAsk(connection.name), text: tr.egress.removeText, ok: tr.egress.remove, danger: true }).then(
                                (yes) => {
                                    if (yes) void run('remove', () => vpn.call('remove', { connection: connection.id }));
                                },
                            )
                        }
                    >
                        {tr.egress.remove}
                    </button>
                </div>
            )}
        </section>
    );
}

function SubsTab({ vpn }: { vpn: Vpn }) {
    const panel = usePanel();
    const [adding, setAdding] = useState(false);
    const connections = vpn.status?.connections ?? [];
    const full = connections.length >= (vpn.status?.max ?? 8);
    if (!vpn.view.app)
        return (
            <div class="vpn-notice">
                <p class="m-0">{tr.egress.noApp}</p>
                <Action label={tr.egress.openMagicPlace} onClick={() => panel.go('magicplace', 'store', 'own-server')} />
            </div>
        );
    return (
        <div class="vpn-tab">
            {vpn.statusProblem && <p class="vpn-note c-dot-offline">{vpn.statusProblem}</p>}
            {connections.map((connection) => (
                <ConnectionCard key={connection.id} vpn={vpn} connection={connection} />
            ))}
            {adding ? (
                <AddForm vpn={vpn} onClose={() => setAdding(false)} />
            ) : (
                vpn.mayEdit && (
                    <div>
                        <Action label={tr.egress.add} tone="on" disabled={!vpn.status || full} onClick={() => setAdding(true)} />
                    </div>
                )
            )}
            <Group>
                <div class="vpn-card-pad vpn-about">
                    <p class="m-0">{tr.egress.subsIntro}</p>
                    <dl class="vpn-facts">
                        <dt>{tr.egress.protocols}</dt>
                        <dd>
                            <ul class="vpn-chips">
                                {PROTOCOLS.map((name) => (
                                    <li key={name}>{name}</li>
                                ))}
                            </ul>
                        </dd>
                        <dt>{tr.egress.subscriptions}</dt>
                        <dd class="vpn-note">{tr.egress.subscriptionsAbout}</dd>
                    </dl>
                </div>
            </Group>
        </div>
    );
}

// ─── Экран ───────────────────────────────────────────────────────────────────────

/** «VPN» — раздел «Настроек» и окно, куда ведут приложение «VPN» и его виджет. */
export function EgressScreen() {
    const { vpn, problem } = useVpn();
    const [tab, setTab] = useState<Tab>(() => {
        const asked = new URLSearchParams(location.hash.split('?')[1] ?? '').get('tab');
        return asked === 'places' || asked === 'settings' || asked === 'subs' ? asked : 'home';
    });
    useEffect(() => {
        if (vpn?.view.rows.some((row) => row.new)) void api.egressSeen().catch(() => undefined);
        // Один раз за открытие: «новое» увидено.
        // eslint-disable-next-line react-hooks/exhaustive-deps
    }, [!!vpn]);
    if (!vpn)
        return (
            <section class="card glass vpn-card">
                <p class="vpn-note vpn-card-pad">{problem ?? tr.egress.loading}</p>
            </section>
        );
    if (vpn.appOff) return <AppOff vpn={vpn} problem={problem} />;
    const tabs = [
        { id: 'home' as const, label: tr.egress.tabHome, icon: <IconPower /> },
        { id: 'places' as const, label: tr.egress.tabPlaces, icon: <IconSpark /> },
        { id: 'settings' as const, label: tr.egress.tabSettings, icon: <IconShield /> },
        { id: 'subs' as const, label: tr.egress.tabSubs, icon: <IconCheck /> },
    ];
    return (
        <div class="vpn-screen">
            <SectionTabs label={tr.egress.tabs} tabs={tabs} tab={tab} onPick={setTab} />
            {problem && (
                <p class="vpn-note c-dot-offline" role="alert">
                    {problem}
                </p>
            )}
            {tab === 'home' && <HomeTab vpn={vpn} go={setTab} />}
            {tab === 'places' && <PlacesTab vpn={vpn} />}
            {tab === 'settings' && <SettingsTab vpn={vpn} />}
            {tab === 'subs' && <SubsTab vpn={vpn} />}
        </div>
    );
}

/**
 * Приложение VPN выключено: одна карточка — «VPN выключен», подписки на месте, одна кнопка включает приложение. Без
 * вкладок: без программы ни места, ни подписки не поменять, а «Нет подписки» пугало, будто подписка пропала.
 */
function AppOff({ vpn, problem }: { vpn: Vpn; problem: string | null }) {
    const [busy, setBusy] = useState(false);
    const [error, setError] = useState<string | null>(null);
    const may = can('plugins.manage');
    const on = async (): Promise<void> => {
        if (!vpn.view.app) return;
        setBusy(true);
        setError(null);
        try {
            await api.enablePackage(vpn.view.app);
            vpn.reload();
        } catch (e) {
            setError(problemOf(e));
        } finally {
            setBusy(false);
        }
    };
    return (
        <section class="card glass vpn-hero vpn-hero-off vpn-off-card" aria-live="polite">
            <p class="vpn-state-title">{tr.egress.appOffTitle}</p>
            <p class="vpn-note m-0">{tr.egress.appOffAbout}</p>
            {may ? (
                <Action label={tr.egress.appOffTurnOn} tone="on" busy={busy} onClick={() => void on()} />
            ) : (
                <p class="vpn-note m-0">{tr.egress.appOffAdmin}</p>
            )}
            {(error ?? problem) && (
                <p class="vpn-note c-dot-offline m-0" role="alert">
                    {error ?? problem}
                </p>
            )}
        </section>
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
