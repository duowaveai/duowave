// Встроенные виджеты стола (docs/plans/desk.md): то, что уже знает пульт, — режим колонки, последние вопросы, кто
// сколько ответил, чего пока не умеем; у помощника — как он присматривает за колонкой и его мастерская; у шлюза 0.11 на
// колонке — его компьютер прежней карточкой. Что компьютер даёт другим шлюзам — screens/helper-pc.tsx, «Где сейчас» —
// screens/helpers.tsx (docs/plans/home-and-helpers.md, блок 1). Регистрируются в слот `desk.widgets` (builtin.tsx) тем
// же путём, что виджеты плагинов.

import { useEffect, useRef, useState } from 'preact/hooks';

import { ApiError, DraftView, PlacementView, RequestView, WorkView, api, can, problemOf } from '../api';
import { CAPABILITY_TITLES } from '../capabilities';
import { SETTLE_MS, usePoll } from '../poll';
import { useSettledVersion } from '../shell/agent-plaque';
import { usePanel } from '../shell/context';
import { stationOfferOf, stationUpdateStep } from '../station-update';
import { timeAgo } from '../time';
import { useWidgetEmpty } from '../ui/widget';
import { VIA } from '../via';
import { AllocPanel, ModeCard } from './home';
import { readStationRequests } from './station-widgets';
import { feltMs, seconds } from './trip';
import { dotOf, endWord, kindWord, progressLine, useWorks } from './works';
import { locale, tr } from '../i18n';
import { byMode } from '../words';

/** Строка «Слышит · Думает · Говорит» на сцене стола: её раскрывает «подробнее» у режима. Общая для сцены и виджета. */
let whoOpen = false;
const whoListeners = new Set<() => void>();
export function useWhoOpen(): [boolean, () => void] {
    const [, rerender] = useState(0);
    useEffect(() => {
        const update = () => rerender((n) => n + 1);
        whoListeners.add(update);
        return () => {
            whoListeners.delete(update);
        };
    }, []);
    return [
        whoOpen,
        () => {
            whoOpen = !whoOpen;
            for (const listener of [...whoListeners]) listener();
        },
    ];
}

/** «Кто отвечает»: Яндекс, Шлюз или Свой — сегментом, с объяснением режима. */
export function ModeWidget() {
    const panel = usePanel();
    const [open, toggle] = useWhoOpen();
    return <ModeCard health={panel.health} onOpenSettings={() => panel.go('character', 'voice')} detailsOpen={open} onToggleDetails={toggle} />;
}

const hhmm = (iso: string): string => new Date(iso).toLocaleTimeString(locale(), { hour: '2-digit', minute: '2-digit' });

/**
 * Через проход отказала сама колонка, а не проход компьютера. Тело ошибки шлюза называет путь, как его видел тот, кто
 * отказал (platform/http/api.rs, error_bodies): у прохода это /station/<колонка>/…, у колонки — её /api/v1/….
 */
function refusedByStation(error: ApiError): boolean {
    try {
        const path = (JSON.parse(error.body) as { path?: unknown } | null)?.path;
        return typeof path === 'string' && !path.startsWith('/station/');
    } catch {
        return false;
    }
}

/**
 * «Последние вопросы»: время, что спросили, через сколько колонка заговорила. Весь журнал — по нажатию. У помощника —
 * вопросы колонки из её журнала, через проход; колонка даёт человеку права его роли, и журнал свой она показывает
 * только администратору (403) — виджет так и говорит. Отказал сам проход компьютера (пульт открыт не из дома, нет
 * права смотреть колонку) — виджет говорит его словами.
 */
export function RecentWidget() {
    const panel = usePanel();
    const device = panel.current?.device;
    const station = panel.role.role === 'helper' ? panel.role.station?.key : undefined;
    const [stationList, setStationList] = useState<RequestView[] | null>(null);
    /** Почему журнал колонки не прочитать (403); null — читается. */
    const [refused, setRefused] = useState<string | null>(null);
    // Раз в 15 с и по переменам — без наложений (poll.ts): проход к колонке, что не отвечает, ждёт ответа до 8 с.
    usePoll({
        on: !!station,
        deps: [station],
        every: 15_000,
        version: panel.version,
        read: (signal) =>
            readStationRequests(station ?? '', signal)
                .then((fresh) => {
                    if (signal.aborted) return;
                    setStationList(fresh);
                    setRefused(null);
                })
                .catch((e) => {
                    if (signal.aborted || !(e instanceof ApiError) || e.status !== 403) return;
                    setRefused(refusedByStation(e) ? tr.desk.historyAdminOnly : problemOf(e));
                }),
    });
    const list = station
        ? (stationList ?? []).slice(0, 4)
        : panel.feed.requests.filter((request) => !device || request.device === device).slice(0, 4);
    // Вопросов ещё не было — виджета не видно (hideWhileEmpty): «спросите что-нибудь» уже сказано под колонкой. Отказ
    // колонки показать журнал — не пустота: виджет так и говорит.
    useWidgetEmpty(list.length === 0 && !(station && refused));
    if (station && refused) return <p class="w-cap m-0">{refused}</p>;
    if (list.length === 0) return <p class="w-cap m-0">{byMode(tr.desk.askStation, tr.desk.askDevice)}</p>;
    // У помощника это вопросы колонки — и журнал её: окно колонки, а не журнал компьютера, где их нет.
    const openJournal = () => (station ? panel.go('settings', 'history', undefined, { on: 'station' }) : panel.go('settings', 'history'));
    return (
        <div class="w-rows">
            {list.map((request) => {
                const ms = request.status === 'active' ? undefined : feltMs(request);
                return (
                    <button key={request.id} class="w-row w-row-btn" onClick={openJournal}>
                        <span class="w-cap w-time">{hhmm(request.startedAt)}</span>
                        <span class="w-text">{request.text ?? (request.kind === 'say' ? tr.desk.saidItself : tr.desk.voiceQuestion)}</span>
                        <span class="w-cap">{request.status === 'active' ? tr.desk.going : ms !== undefined ? seconds(ms) : ''}</span>
                    </button>
                );
            })}
        </div>
    );
}

/** «Кто сколько ответил»: полоса своё / Яндекс / выключено и сколько обслужено с запуска. Своя карточка. */
export function AllocWidget() {
    const panel = usePanel();
    return <AllocPanel health={panel.health} catalog={panel.catalog} />;
}

/** «Пока не умеем»: что спросили, а ответить было некому. */
export function UnsupportedWidget() {
    const panel = usePanel();
    const list = panel.debug.unsupported;
    // Живой виджет (docs/plans/start-desk.md): появляется, когда есть вопросы без ответа.
    useWidgetEmpty(list.length === 0);
    if (list.length === 0) return <p class="w-cap m-0">{tr.desk.allSupported}</p>;
    return (
        <div class="w-rows">
            {list.slice(0, 3).map((entry) => (
                <button key={entry.at + entry.text} class="w-row w-row-btn w-row-col" onClick={() => panel.go('settings', 'history')}>
                    <span class="w-text">{tr.desk.quoted(entry.text)}</span>
                    <span class="w-cap">
                        {timeAgo(entry.at)} · {entry.reason}
                    </span>
                </button>
            ))}
        </div>
    );
}

// ─── Помощник ──────────────────────────────────────────────────────────────────────────────────────────────────────

/**
 * Размещение глазами компьютера — одно на все виджеты помощника: читается раз в полминуты и по переменам — не чаще
 * раза в несколько секунд и одним запросом на всех.
 */
let placement: PlacementView | null = null;
let placementAt = 0;
let placementVersion = -1;
let reading: Promise<void> | null = null;
let placementTimer = 0;
const placementListeners = new Set<() => void>();

function readPlacement(): Promise<void> {
    reading ??= api
        .placement()
        .then((view) => {
            placement = view;
        })
        .catch(() => undefined)
        .finally(() => {
            placementAt = Date.now();
            reading = null;
            for (const listener of [...placementListeners]) listener();
        });
    return reading;
}

/** Перечитать размещение, но не раньше, чем через несколько секунд от прошлого чтения. */
function readPlacementSoon(): void {
    if (placementTimer) return;
    const wait = placementAt + SETTLE_MS - Date.now();
    if (wait <= 0) return void readPlacement();
    placementTimer = window.setTimeout(() => {
        placementTimer = 0;
        void readPlacement();
    }, wait);
}

export function usePlacementView(): [PlacementView | null, () => Promise<void>] {
    const panel = usePanel();
    const [, rerender] = useState(0);
    useEffect(() => {
        const update = () => rerender((n) => n + 1);
        placementListeners.add(update);
        // Виджетов несколько, а чтение — одно: каждый лишь напоминает, если прочитанное старше полуминуты, а пока колонка
        // обновляется — старше трёх секунд: её путь (копия, загрузка, перезапуск) идёт около минуты, и итог виден сразу.
        const id = window.setInterval(() => Date.now() - placementAt >= (anyUpdating() ? 3_000 : 30_000) && void readPlacement(), 3_000);
        return () => {
            placementListeners.delete(update);
            window.clearInterval(id);
        };
    }, []);
    useEffect(() => {
        if (placementVersion !== panel.version || Date.now() - placementAt > 30_000) {
            placementVersion = panel.version;
            readPlacementSoon();
        }
    }, [panel.version]);
    return [placement, readPlacement];
}

/**
 * Новая версия для колонки, которую обновляет этот компьютер (пульт Турбо, owner 17.11): точка у «Настроек» и у
 * «Обновлений и копий» — как у собственной версии, иначе о ней не узнать, не зайдя туда. `on` — только в пульте Турбо:
 * в других пультах размещение не читается ради точки.
 */
export function useStationOffer(on: boolean): string | null {
    const panel = usePanel();
    const [, rerender] = useState(0);
    useEffect(() => {
        if (!on) return;
        const update = () => rerender((n) => n + 1);
        placementListeners.add(update);
        if (Date.now() - placementAt > 30_000) readPlacementSoon();
        const id = window.setInterval(() => Date.now() - placementAt >= 60_000 && void readPlacement(), 30_000);
        return () => {
            placementListeners.delete(update);
            window.clearInterval(id);
        };
    }, [on]);
    if (!on || !can('system.update')) return null;
    const station = stationOf(placement, panel.current?.device);
    // Выпуск новее компьютера — тоже новая версия для колонки: её получат через компьютер (owner 20.11).
    return station && !station.live.updating ? stationOfferOf(stationUpdateStep(station)) : null;
}

/** Какая-нибудь колонка сейчас обновляется с этого компьютера. */
function anyUpdating(): boolean {
    return !!placement && [placement.station, ...(placement.stations ?? [])].some((entry) => entry?.live.updating);
}

/** Выбранная колонка помощника в его размещении. */
function stationOf(view: PlacementView | null, device?: string) {
    if (!view) return undefined;
    return (device ? view.stations?.find((entry) => entry.record?.device === device) : undefined) ?? view.station;
}

const mb = (kb?: number): string => (kb === undefined ? '—' : `${Math.round(kb / 1024)}`);

/** «сегодня 14:02», «вчера 22:10», «08.11 09:30» — когда снята копия. */
function copyDay(at: string): string {
    const date = new Date(at);
    const time = date.toLocaleTimeString(locale(), { hour: '2-digit', minute: '2-digit' });
    const day = new Date();
    if (date.toDateString() === day.toDateString()) return tr.desk.today(time);
    day.setDate(day.getDate() - 1);
    if (date.toDateString() === day.toDateString()) return tr.desk.yesterday(time);
    return `${date.toLocaleDateString(locale(), { day: '2-digit', month: '2-digit' })} ${time}`;
}

/**
 * «Присматриваю за колонкой»: копия настроек, обновления, страховка, память. «Открыть» — всё о размещении: на столе —
 * переход в «Колонку», на странице «Этот компьютер» (`onOpen`) — блок «Где работает DuoWave» под карточкой, ведь
 * пульт колонки вокруг неё видит размещение глазами колонки, а копии, перенос и ошибки обновления — у компьютера.
 */
export function WatchWidget() {
    return <Watch />;
}

/** Сама карточка присмотра; `onOpen` — своё «Открыть» вместо перехода в «Колонку». */
export function Watch({ onOpen, opened = false }: { onOpen?: () => void; opened?: boolean }) {
    const panel = usePanel();
    const [view, reread] = usePlacementView();
    const [busy, setBusy] = useState<string | null>(null);
    const [note, setNote] = useState<string | null>(null);
    const station = stationOf(view, panel.current?.device);
    if (!station) return <p class="w-cap m-0">{view ? tr.desk.noOwnGateway : tr.desk.reading}</p>;
    const health = station.live.health?.placement;
    // The home's copy here (docs/plans/home-and-devices.md, stage 3): taken at each change of the centre.
    const copied = station.copiedAt;
    const memory = health?.memory;
    const mayManage = can('stations.manage');
    const updating = station.live.updating;
    const step = stationUpdateStep(station);
    const act = (what: string, run: () => Promise<unknown>, done?: string) => {
        setBusy(what);
        setNote(null);
        void run()
            .then(() => {
                setNote(done ?? null);
                return reread();
            })
            .catch((e) => setNote(problemOf(e)))
            .finally(() => setBusy(null));
    };
    return (
        <div class="flex flex-col gap-8">
            <div class="w-rows">
                <div class="w-row">
                    <span class="w-text">{tr.desk.settingsCopy}</span>
                    <span class="w-cap">{copied ? `${copyDay(copied)} ✓` : tr.desk.none}</span>
                </div>
                <div class="w-row">
                    <span class="w-text">{tr.desk.updates}</span>
                    <span class="w-cap">
                        {step?.kind === 'stationAhead'
                            ? tr.desk.newerThanComputer(step.station)
                            : stationOfferOf(step)
                              ? tr.desk.updateAvailable(stationOfferOf(step) ?? '')
                              : step?.kind === 'follows'
                                ? tr.desk.updateAvailable(step.version)
                                : tr.desk.latest(station.version ?? '—')}
                    </span>
                </div>
                <div class="w-row">
                    <span class="w-text">{tr.desk.safety}</span>
                    <span class="w-cap">{health?.safety === false ? tr.desk.safetyOff : tr.desk.safetyOn}</span>
                </div>
                <div class="w-row">
                    <span class="w-text">{tr.desk.ram}</span>
                    <span class="w-cap">
                        {memory?.rssKb !== undefined ? (memory.totalKb ? tr.desk.mbOf(mb(memory.rssKb), mb(memory.totalKb)) : tr.desk.mbUsed(mb(memory.rssKb))) : '—'}
                    </span>
                </div>
            </div>
            {station.warnings.map((warning) => (
                <p key={warning.kind} class="w-cap c-pill-yandex-fg m-0">
                    {warning.text}
                </p>
            ))}
            {step?.kind === 'blocked' && <p class="w-cap m-0">{step.why}</p>}
            {step?.kind === 'follows' && !updating && (
                <p class="w-cap m-0" role="status">
                    {tr.desk.stationFollows(step.version)}
                </p>
            )}
            <div class="w-actions">
                {mayManage && (step?.kind === 'send' || step?.kind === 'computerFirst') && (
                    <button
                        class="w-btn w-btn-ink"
                        disabled={busy !== null || updating}
                        onClick={() => act('update', () => api.updateStationGateway(station.key))}
                    >
                        {busy === 'update' || updating
                            ? tr.desk.updating
                            : step.kind === 'send'
                              ? tr.desk.updateTo(step.version)
                              : tr.desk.updateBoth(step.version)}
                    </button>
                )}
                <button class="w-btn" aria-expanded={onOpen ? opened : undefined} onClick={() => (onOpen ? onOpen() : panel.go('settings', 'station'))}>
                    {opened ? tr.desk.collapse : tr.desk.open}
                </button>
            </div>
            {/* Обновление идёт в фоне: что с ним — из размещения компьютера, а не из ответа на нажатие. */}
            {updating ? (
                <p class="w-cap m-0" role="status">
                    {tr.desk.stationUpdating}
                </p>
            ) : station.live.updateError ? (
                <p class="w-cap c-pill-yandex-fg m-0" role="status">
                    {tr.desk.stationUpdateFailed(station.live.updateError)}
                </p>
            ) : (
                note && (
                    <p class="w-cap m-0" role="status">
                        {note}
                    </p>
                )
            )}
        </div>
    );
}

/** «Мастерская»: последний черновик — что это, какая версия, что на колонке; «Открыть» — окно мастерской. */
export function WorkshopWidget() {
    const panel = usePanel();
    // Каждый шаг агента — перемена (workshop.changed): список черновиков — раз на пачку перемен.
    const version = useSettledVersion();
    const [drafts, setDrafts] = useState<DraftView[] | null>(null);
    usePoll({
        version,
        settle: 1000,
        read: (signal) =>
            api
                .workshop(signal)
                .then((view) => !signal.aborted && setDrafts(view.drafts))
                .catch(() => !signal.aborted && setDrafts([])),
    });
    const open = (id?: string) => panel.go('skills', 'workshop', id);
    if (!drafts) return <p class="w-cap m-0">{tr.desk.reading}</p>;
    const draft = drafts[0];
    if (!draft)
        return (
            <div class="flex flex-col gap-8">
                <p class="w-cap m-0">{tr.desk.noDrafts}</p>
                <div class="w-actions">
                    <button class="w-btn" onClick={() => open()}>
                        {tr.desk.open}
                    </button>
                </div>
            </div>
        );
    return (
        <div class="flex flex-col gap-8">
            <div class="w-rows">
                {drafts.slice(0, 2).map((entry) => (
                    <button key={entry.id} class="w-row w-row-btn w-row-col" onClick={() => open(entry.id)}>
                        <span class="w-text">
                            {entry.title ?? entry.id} <span class="w-cap">{entry.version ?? ''}</span>
                        </span>
                        <span class="w-cap">
                            {entry.trial
                                ? tr.desk.trialUntil(hhmm(entry.trial.until))
                                : entry.problems.length > 0
                                  ? tr.desk.draftWithErrors
                                  : entry.installed
                                    ? tr.desk.installedVersion(entry.installed)
                                    : tr.desk.draft}
                        </span>
                    </button>
                ))}
            </div>
            <div class="w-actions">
                <button class="w-btn" onClick={() => open(draft.id)}>
                    {tr.desk.open}
                </button>
            </div>
        </div>
    );
}

/**
 * «Работы» на столе (docs/plans/router-and-works.md, решение 17): «Идёт 2, ждёт ответа 1» и по строке на работу — что
 * идёт (чем занят агент, где фоновое дело), что ждёт в очереди и что спрашивает. Нажатие на строку — эта работа,
 * «Все работы» — страница. Ничего не идёт — последняя законченная.
 */
export function WorksWidget() {
    const panel = usePanel();
    const { works, problem } = useWorks(['going', 'queued', 'asking']);
    const [last, setLast] = useState<WorkView | null>(null);
    const idle = !!works && works.length === 0;
    usePoll({
        on: idle,
        version: panel.version,
        read: (signal) =>
            api
                .worksIn(['recent'], signal)
                .then((list) => !signal.aborted && setLast(list[0] ?? null))
                .catch(() => undefined),
    });
    // Работ не было ни одной (ни идущих, ни недавних) — виджета не видно (hideWhileEmpty); с первой работой он появится
    // сам. Пока читаем — тоже не виден: у новичка он не мелькнёт. Появился — остаётся: работа кончилась, а последняя
    // законченная ещё читается, — виджет не должен пропасть на это время.
    const ever = useRef(false);
    if (works?.length || last) ever.current = true;
    useWidgetEmpty(!ever.current);
    const all = () => panel.go('works', '');
    const allButton = (
        <div class="w-actions">
            <button class="w-btn" onClick={all}>
                {tr.router.allWorks}
            </button>
        </div>
    );
    if (!works) return <p class={`w-cap m-0${problem ? ' c-dot-offline' : ''}`}>{problem ?? tr.desk.reading}</p>;
    if (works.length === 0)
        return (
            <div class="flex flex-col gap-8">
                <p class="w-cap m-0">{tr.router.widgetNothing}</p>
                {last && (
                    <button class="w-row w-row-btn" onClick={() => panel.go('works', last.id)}>
                        <span class={dotOf(last)} aria-hidden="true" />
                        <span class="w-text">{tr.router.widgetLast(last.title, endWord(last))}</span>
                    </button>
                )}
                {allButton}
            </div>
        );
    const going = works.filter((work) => work.group === 'going');
    const queued = works.filter((work) => work.group === 'queued');
    const asking = works.filter((work) => work.group === 'asking');
    const rows = [...going, ...asking, ...queued];
    const shown = rows.slice(0, 4);
    const note = (work: WorkView): string =>
        work.group === 'going' ? progressLine(work) || kindWord(work.kind) : work.group === 'asking' ? tr.router.widgetAsking : tr.router.widgetQueuedRow;
    return (
        <div class="flex flex-col gap-8">
            <div class="wk-widget-head">
                <span class="w-strong">{tr.router.widgetSummary(going.length, asking.length)}</span>
                {queued.length > 0 && <span class="w-cap">{tr.router.widgetQueued(queued.length)}</span>}
            </div>
            <div class="w-rows">
                {shown.map((work) => (
                    <button key={work.id} class="w-row w-row-btn wk-widget-row" onClick={() => panel.go('works', work.id)}>
                        <span class={dotOf(work)} aria-hidden="true" />
                        <span class="wk-widget-lines">
                            <span class="w-text">{work.title}</span>
                            <span class="w-cap">{note(work)}</span>
                        </span>
                    </button>
                ))}
            </div>
            {rows.length > shown.length && <p class="w-cap m-0">{tr.router.widgetMore(rows.length - shown.length)}</p>}
            {allButton}
        </div>
    );
}

/** У шлюза на колонке — его помощник: на связи ли он и что даёт; «Открыть помощника» — пульт компьютера. */
export function HelperCardWidget() {
    const panel = usePanel();
    const home = panel.health?.placement?.homeServer;
    // Живой виджет (docs/plans/start-desk.md): только когда с помощником беда — выключен или не на связи.
    useWidgetEmpty(!home?.url || (home.state === 'up' && home.enabled !== false));
    if (!home?.url) return <p class="w-cap m-0">{tr.desk.helperNotHelping}</p>;
    let panelUrl: string | null = null;
    let address: string | null = null;
    try {
        const url = new URL(home.url);
        panelUrl = `${url.origin}/panel`;
        address = url.host;
    } catch {
        panelUrl = null;
    }
    const up = home.state === 'up' && home.enabled !== false;
    // The computer's panel only while the computer answers: a link to a computer that is away leads nowhere. Neither
    // the link nor when it opens in the station's panel seen through this computer (`?via=`): that is the computer's
    // own window — its /panel sends a helper back to it, and its own page there is «Этот компьютер».
    const reachable = home.state === 'up';
    const computerPanel = !VIA && panelUrl;
    return (
        <div class="flex flex-col gap-8">
            <div class="w-row">
                <span class={`w-dot ${up ? 'w-dot-own' : 'w-dot-off'}`} aria-hidden="true" />
                <span class="w-text">
                    {home.enabled === false ? tr.desk.helpDisabled : up ? tr.desk.online : tr.desk.offline}
                    {home.since && <span class="w-cap">{tr.desk.since(hhmm(home.since))}</span>}
                </span>
            </div>
            <p class="w-cap m-0">
                {up
                    ? tr.desk.helperUp
                    : tr.desk.helperDown}
            </p>
            {!reachable && computerPanel && address && <p class="w-cap m-0">{tr.desk.helperAwayStart(address)}</p>}
            {reachable && computerPanel && (
                <div class="w-actions">
                    <a class="w-btn" href={computerPanel} target="_blank" rel="noreferrer">
                        {tr.desk.openComputerPanel}
                    </a>
                </div>
            )}
        </div>
    );
}

/** Подписи возможностей — для виджетов плагинов, которым нужны имена (журнал вопросов). */
export const capabilityTitle = (id: string): string => CAPABILITY_TITLES[id] ?? id;
