// «Состояние дома» (docs/plans/home-setup-flow.md, этап 4; холст «Онбординг», доска HomeStatus): строки центра
// (`GET /api/v1/home/status`) — у каждой одно действие, которое ведёт в готовый экран, — и строка пульта: колонки в сети,
// которых нет в доме (их находит поиск компьютера, центр на колонке не ищет). Один список для последнего шага первых
// шагов (onb/status.tsx) и для карточки на столе, которая прячется, когда всё зелёное.
//
// На столе (замечание 4 после 0.18.1: плашка висела всегда — в доме без Турбо строка «Турбо нет» жёлтая навсегда):
// - крестик сворачивает карточку в одну строку «Состояние дома · 2 замечания ›»; нажатие разворачивает;
// - у жёлтой строки, которая — выбор человека, а не поломка («Турбо нет», «ИИ не выбран» в режиме «Алиса», голос
//   персонажа, колонки в сети не в доме), — «Не нужно»: строка серая, «оставили так», и не держит карточку; ни одной
//   жёлтой или красной, кроме оставленных, — карточки нет совсем, и строки тоже;
// - свёрнутая разворачивается сама, когда появилась жёлтая или красная строка, которой при сворачивании не было (или
//   жёлтая стала красной); оставленная строка, ставшая красной, снова считается.
// Что свернули и что оставили — в памяти пульта у человека (ключ `homeStatus`): по id строки и её цвету, не по тексту
// (у строки Wi-Fi текст меняется).

import { useEffect, useState } from 'preact/hooks';

import { FoundStation, HomeStatusRow, HomeStatusView, api, can, problemOf, stationCall } from '../api';
import { useOwnHealth } from '../home-devices';
import { tr } from '../i18n';
import { usePanel } from '../shell/context';
import { remember, useRecalled } from '../shell/memory';
import { VIA } from '../via';
import { IconClose } from '../icons';
import { AllowFirewall } from './connect';

type Panel = ReturnType<typeof usePanel>;

/** Центр дома, когда он не этот шлюз: колонка, которой этот компьютер — Турбо (её ключ — для прохода). */
function centreOf(panel: Panel): string | undefined {
    return panel.role.role === 'helper' ? panel.role.station?.key : undefined;
}

/** Состояние дома у центра — раз в 20 с, — и колонки в сети не в доме. `centre` — ключ колонки-центра, если известен
 *  раньше, чем пульт узнал роль (компьютер только что вошёл в дом). */
export function useHomeStatus(centre?: string): { view: HomeStatusView | null; error: string | null; found: FoundStation[]; reread: () => void } {
    const panel = usePanel();
    const via = centre ?? centreOf(panel);
    const [view, setView] = useState<HomeStatusView | null>(null);
    const [error, setError] = useState<string | null>(null);
    const [found, setFound] = useState<FoundStation[]>([]);
    const [round, setRound] = useState(0);
    useEffect(() => {
        if (!can('station.view')) return;
        let alive = true;
        const read = (): void =>
            void (via ? stationCall<HomeStatusView>(via, 'GET', '/api/v1/home/status') : api.homeStatus())
                .then((fresh) => {
                    if (!alive) return;
                    setView(fresh);
                    setError(null);
                })
                .catch((e) => alive && setError(problemOf(e)));
        read();
        const timer = window.setInterval(read, 20_000);
        return () => {
            alive = false;
            window.clearInterval(timer);
        };
    }, [via, round]);
    // Колонки в сети, которых нет в доме: ищет компьютер; окно колонки и пульт на колонке не ищут. Пока пульт не знает,
    // где он (/health не прочитан), не ищет никто: поиск читает SSH-приветствие колонок в сети.
    const known = (panel.health?.placement?.stations ?? []).map((station) => station.address).join();
    const role = panel.health?.placement?.role;
    useEffect(() => {
        if (VIA || !can('stations.manage') || role !== 'pc') return;
        let alive = true;
        void api
            .discoverStations()
            .then(({ stations }) => {
                if (!alive) return;
                const inHome = new Set(known.split(',').filter(Boolean));
                setFound(stations.filter((station) => !inHome.has(station.address)));
            })
            .catch(() => undefined);
        return () => {
            alive = false;
        };
    }, [known, role]);
    return { view, error, found, reread: () => setRound((n) => n + 1) };
}

/** Память карточки на столе: строки, оставленные как есть (`id` → действие строки), и что было, когда свернули. */
interface Memo {
    kept?: Record<string, string>;
    folded?: string[];
}
const MEMO = 'homeStatus';

/** Жёлтые строки-выборы, которые можно оставить как есть: по их действию. Поломки (колонка не на связи, брандмауэр,
 *  голос мимо DuoWave, Wi-Fi) — нельзя. */
const KEEPABLE = new Set<string>(['turbo', 'brain', 'voice', 'addStations']);

const keepable = (row: HomeStatusRow): boolean => row.state === 'warn' && !!row.action && KEEPABLE.has(row.action);
const isKept = (row: HomeStatusRow, memo: Memo): boolean => keepable(row) && memo.kept?.[row.id] === row.action;
/** Строка в памяти свёрнутой карточки: id и цвет. */
const sign = (row: HomeStatusRow): string => `${row.id}:${row.state}`;

const MARK: Record<HomeStatusRow['state'], { tone: string; glyph: string; dot: string }> = {
    ok: { tone: 'mark-ok', glyph: '✓', dot: 'dot dot-ok' },
    warn: { tone: 'mark-warn', glyph: '!', dot: 'dot dot-warn' },
    bad: { tone: 'mark-bad', glyph: '✕', dot: 'dot dot-offline' },
};

/** Строки с действиями. `go` — открыть экран пульта (первые шаги закрываются сначала сами). */
export function HomeStatusRows({
    view,
    found,
    centre,
    onFixed,
    go,
    look = 'onb',
}: {
    view: HomeStatusView;
    found: FoundStation[];
    centre?: string;
    onFixed: () => void;
    go: (section: string, tab?: string) => void;
    /** `onb` — список первых шагов (их стиль), `desk` — строки виджета на столе. */
    look?: 'onb' | 'desk';
}) {
    const memo = useRecalled<Memo>(MEMO) ?? {};
    const keep = (row: HomeStatusRow, on: boolean): void => {
        const kept = { ...(memo.kept ?? {}) };
        if (on && row.action) kept[row.id] = row.action;
        else delete kept[row.id];
        remember(MEMO, { ...memo, kept: Object.keys(kept).length ? kept : undefined } satisfies Memo);
    };
    const panel = usePanel();
    const t = tr.onboarding;
    const via = centre ?? centreOf(panel);
    const [busy, setBusy] = useState<string | null>(null);
    const [note, setNote] = useState<{ id: string; text: string } | null>(null);
    const [error, setError] = useState<string | null>(null);
    const rows: HomeStatusRow[] = [...view.rows];
    if (found.length > 0) rows.push({ id: 'stations', state: 'warn', text: t.statusFound(found.length), action: 'addStations' });
    // Брандмауэр закрывает вход к Турбо: кнопка — там, где пульт открыт на самом компьютере-Турбо. Турбо в доме может
    // быть несколько (22.11): строка говорит, чей брандмауэр, и кнопка — только на том компьютере.
    const ownId = useOwnHealth()?.gateway?.id;
    const onThisComputer = !VIA && panel.health?.placement?.role !== 'station';
    // Центр не сказал, чей брандмауэр (до 0.18.2): в окне дома — этот компьютер, раз он Турбо; кнопка сама спрячется не
    // на Windows.
    const onTurboOf = (row: HomeStatusRow): boolean => (row.turbo ? !!ownId && row.turbo === ownId : VIA ? !!ownId : onThisComputer);
    const act = async (row: HomeStatusRow): Promise<void> => {
        setNote(null);
        setError(null);
        switch (row.action) {
            case 'pointLocal':
                setBusy(row.id);
                try {
                    await (via ? stationCall(via, 'POST', '/api/v1/placement/point-local', {}) : api.pointLocal());
                    onFixed();
                } catch (e) {
                    setError(problemOf(e));
                } finally {
                    setBusy(null);
                }
                return;
            case 'brain':
                return go('character', 'brain');
            case 'voice':
                return go('character', 'voice');
            case 'mode':
                return go('home');
            case 'station':
                return panel.openConnect();
            case 'addStations':
                return go('settings', 'devices');
            case 'turbo':
                return setNote({ id: row.id, text: t.statusTurboHow });
            case 'firewall':
                if (!onTurboOf(row)) setNote({ id: row.id, text: t.statusFirewallThere });
                return;
            // Голос персонажа не пускает защита Windows (22.11): что делать — решает человек, с предупреждением.
            case 'windowsVoice':
                return setNote({ id: row.id, text: onTurboOf(row) ? t.statusWindowsVoice : t.statusWindowsVoiceThere });
        }
    };
    // «Открыть настройки Windows» — на том самом компьютере, после объяснения.
    const openWindows = (row: HomeStatusRow) =>
        row.action === 'windowsVoice' &&
        note?.id === row.id &&
        onTurboOf(row) && (
            <button
                type="button"
                class={look === 'desk' ? 'w-btn' : 'btn-ghost'}
                disabled={busy !== null}
                aria-busy={busy === row.id || undefined}
                onClick={() => {
                    setBusy(row.id);
                    setError(null);
                    api.openWindowsSecurity()
                        .catch((e) => setError(problemOf(e)))
                        .finally(() => setBusy(null));
                }}
            >
                {t.statusWindowsVoiceOpen}
            </button>
        );
    const fix = (row: HomeStatusRow) =>
        row.state !== 'ok' &&
        row.action &&
        !(row.action === 'firewall' && onTurboOf(row)) && (
            <button type="button" class={look === 'desk' ? 'w-btn' : 'btn-ghost'} disabled={busy !== null} aria-busy={busy === row.id || undefined} onClick={() => void act(row)}>
                {t.statusFix[row.action]}
            </button>
        );
    if (look === 'desk')
        return (
            <>
                <div class="w-rows">
                    {rows.map((row) => (
                        <div key={row.id} class="flex flex-col gap-6">
                            <div class={`w-row${isKept(row, memo) ? ' is-kept' : ''}`}>
                                <span class={isKept(row, memo) ? 'dot' : MARK[row.state].dot} aria-hidden="true" />
                                {/* Строка — целая фраза: переносится, а не обрезается многоточием. */}
                                <span class="flex-1 min-w-0 w-text" style={{ whiteSpace: 'normal' }}>
                                    {row.text}
                                    {isKept(row, memo) && <span class="w-cap"> · {t.statusKept}</span>}
                                </span>
                                {isKept(row, memo) ? (
                                    <button type="button" class="w-btn" onClick={() => keep(row, false)}>
                                        {t.statusKeepUndo}
                                    </button>
                                ) : (
                                    <>
                                        {fix(row)}
                                        {keepable(row) && (
                                            <button type="button" class="w-btn" title={t.statusKeepNote} onClick={() => keep(row, true)}>
                                                {t.statusKeep}
                                            </button>
                                        )}
                                    </>
                                )}
                            </div>
                            {row.state !== 'ok' && row.action === 'firewall' && onTurboOf(row) && <AllowFirewall onAllowed={onFixed} />}
                            {note?.id === row.id && <p class="w-cap m-0">{note.text}</p>}
                            {openWindows(row)}
                        </div>
                    ))}
                </div>
                {error && (
                    <p class="w-cap m-0 c-dot-offline" role="alert">
                        {error}
                    </p>
                )}
            </>
        );
    return (
        <>
            <ul class="checks">
                {rows.map((row) => (
                    <li key={row.id}>
                        <span class={`mark ${MARK[row.state].tone}`} aria-hidden="true">
                            {MARK[row.state].glyph}
                        </span>
                        <span class="flex-1 min-w-0 flex flex-col gap-6">
                            <span class="s">{row.text}</span>
                            {row.state !== 'ok' && row.action === 'firewall' && onTurboOf(row) && <AllowFirewall onAllowed={onFixed} />}
                            {note?.id === row.id && <span class="small">{note.text}</span>}
                            {openWindows(row)}
                        </span>
                        {fix(row)}
                    </li>
                ))}
            </ul>
            {error && (
                <p class="small" role="alert" style={{ margin: 0, color: 'var(--bad)' }}>
                    {error}
                </p>
            )}
        </>
    );
}

/** Карточка на столе: пока в доме есть жёлтое или красное, кроме оставленного как есть; свёрнутая — одна строка. */
export function HomeStatusCard() {
    const panel = usePanel();
    const t = tr.onboarding;
    const { view, found, reread } = useHomeStatus();
    const memo = useRecalled<Memo>(MEMO) ?? {};
    if (!view) return null;
    const rows: HomeStatusRow[] = [...view.rows];
    if (found.length > 0) rows.push({ id: 'stations', state: 'warn', text: t.statusFound(found.length), action: 'addStations' });
    const open = rows.filter((row) => row.state !== 'ok' && !isKept(row, memo));
    if (open.length === 0) return null;
    // Свёрнута — пока нет жёлтого или красного, которого при сворачивании не было.
    const folded = !!memo.folded && open.every((row) => memo.folded?.includes(sign(row)));
    const worst = open.some((row) => row.state === 'bad') ? 'bad' : 'warn';
    if (folded)
        return (
            <button
                type="button"
                class="card glass home-status home-status-line"
                aria-label={t.statusUnfold}
                onClick={() => remember(MEMO, { ...memo, folded: undefined } satisfies Memo)}
            >
                <span class={MARK[worst].dot} aria-hidden="true" />
                <span class="flex-1 min-w-0 text-left">{t.statusLine(open.length)}</span>
                <span aria-hidden="true">›</span>
            </button>
        );
    return (
        <section class="card glass px-16 py-14 flex flex-col gap-10 home-status" aria-label={t.statusCard}>
            <div class="flex items-center gap-8">
                <h2 class="t-title-xs m-0 flex-1">{t.statusCard}</h2>
                <button
                    type="button"
                    class="home-status-fold"
                    aria-label={t.statusFold}
                    title={t.statusFold}
                    onClick={() => remember(MEMO, { ...memo, folded: open.map(sign) } satisfies Memo)}
                >
                    <IconClose size={14} />
                </button>
            </div>
            <HomeStatusRows view={view} found={found} onFixed={reread} go={(section, tab) => panel.go(section, tab)} look="desk" />
        </section>
    );
}
