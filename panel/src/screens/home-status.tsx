// «Состояние дома» (docs/plans/home-setup-flow.md, этап 4; холст «Онбординг», доска HomeStatus): строки центра
// (`GET /api/v1/home/status`) — у каждой одно действие, которое ведёт в готовый экран, — и строка пульта: колонки в сети,
// которых нет в доме (их находит поиск компьютера, центр на колонке не ищет). Один список для последнего шага первых
// шагов (onb/status.tsx) и для карточки на столе, которая прячется, когда всё зелёное.

import { useEffect, useState } from 'preact/hooks';

import { FoundStation, HomeStatusRow, HomeStatusView, api, can, problemOf, stationCall } from '../api';
import { tr } from '../i18n';
import { usePanel } from '../shell/context';
import { VIA } from '../via';
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
    const panel = usePanel();
    const t = tr.onboarding;
    const via = centre ?? centreOf(panel);
    const [busy, setBusy] = useState<string | null>(null);
    const [note, setNote] = useState<{ id: string; text: string } | null>(null);
    const [error, setError] = useState<string | null>(null);
    const rows: HomeStatusRow[] = [...view.rows];
    if (found.length > 0) rows.push({ id: 'stations', state: 'warn', text: t.statusFound(found.length), action: 'addStations' });
    // Брандмауэр закрывает вход к Турбо: кнопка — там, где пульт открыт на самом компьютере-Турбо.
    const onTurbo = !VIA && panel.health?.placement?.role !== 'station';
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
                if (!onTurbo) setNote({ id: row.id, text: t.statusFirewallThere });
                return;
        }
    };
    const fix = (row: HomeStatusRow) =>
        row.state !== 'ok' &&
        row.action &&
        !(row.action === 'firewall' && onTurbo) && (
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
                            <div class="w-row">
                                <span class={MARK[row.state].dot} aria-hidden="true" />
                                {/* Строка — целая фраза: переносится, а не обрезается многоточием. */}
                                <span class="flex-1 min-w-0 w-text" style={{ whiteSpace: 'normal' }}>
                                    {row.text}
                                </span>
                                {fix(row)}
                            </div>
                            {row.state !== 'ok' && row.action === 'firewall' && onTurbo && <AllowFirewall onAllowed={onFixed} />}
                            {note?.id === row.id && <p class="w-cap m-0">{note.text}</p>}
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
                            {row.state !== 'ok' && row.action === 'firewall' && onTurbo && <AllowFirewall onAllowed={onFixed} />}
                            {note?.id === row.id && <span class="small">{note.text}</span>}
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

/** Карточка на столе: пока в доме не всё зелёное. */
export function HomeStatusCard() {
    const panel = usePanel();
    const { view, found, reread } = useHomeStatus();
    if (!view || (view.green && found.length === 0)) return null;
    return (
        <section class="card glass px-16 py-14 flex flex-col gap-10 home-status" aria-label={tr.onboarding.statusCard}>
            <h2 class="t-title-xs m-0">{tr.onboarding.statusCard}</h2>
            <HomeStatusRows view={view} found={found} onFixed={reread} go={(section, tab) => panel.go(section, tab)} look="desk" />
        </section>
    );
}
