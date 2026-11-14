// Встроенные виджеты стола (docs/plans/desk.md): то, что уже знает пульт, — режим колонки, последние вопросы, кто
// сколько ответил, чего пока не умеем; у помощника — что он даёт колонке, как присматривает за ней и его мастерская;
// у шлюза на колонке — его помощник. Регистрируются в слот `desk.widgets` (builtin.tsx) тем же путём, что виджеты
// плагинов.

import { useEffect, useState } from 'preact/hooks';
import { useDevMode } from '../shell/dev';

import { ApiError, DraftView, HelperPart, HelperServed, PlacementView, RequestView, api, can, problemOf } from '../api';
import { CAPABILITY_TITLES } from '../capabilities';
import { SETTLE_MS, usePoll } from '../poll';
import { useSettledVersion } from '../shell/agent-plaque';
import { usePanel } from '../shell/context';
import { timeAgo } from '../time';
import { Switch } from '../ui/switch';
import { AllocPanel, ModeCard } from './home';
import { readStationRequests } from './station-widgets';
import { feltMs, seconds } from './trip';
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
    if (station && refused) return <p class="w-cap m-0">{refused}</p>;
    const list = station
        ? (stationList ?? []).slice(0, 4)
        : panel.feed.requests.filter((request) => !device || request.device === device).slice(0, 4);
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

/** «214 вопросов», «198 фраз», «31 ответ» — слова и числа по языку. */
const parts = (): Array<{ id: HelperPart; title: string; served: (n: number) => string; without: string }> => [
    { id: 'asr', title: tr.desk.partEar, served: tr.desk.questionsCount, without: tr.desk.withoutCloud },
    { id: 'tts', title: tr.desk.partVoice, served: tr.desk.phrasesCount, without: tr.desk.withoutAlice },
    { id: 'llm', title: tr.desk.partModel, served: tr.desk.answersCount, without: tr.desk.withoutAlice },
    // Бот Telegram этого компьютера для колонки, в обе стороны: «Передай маме, что…» и сообщения контактов. У колонки
    // своего бота нет. Один переключатель на оба направления; счёт — тоже за оба.
    { id: 'messages', title: tr.desk.partMessages, served: tr.desk.messagesCount, without: tr.desk.withoutNobody },
];

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
 * «Что даю колонке» (docs/plans/desk.md, этап 4; макет SpaceHelper): помощь целиком и по отдельности — слух, голос,
 * модель, сообщения — со счётом за сегодня и тем, чем колонка обходится без компьютера.
 */
export function HelperWidget() {
    const panel = usePanel();
    const dev = useDevMode();
    const [view, reread] = usePlacementView();
    const [served, setServed] = useState<HelperServed | null>(null);
    const [busy, setBusy] = useState<string | null>(null);
    const [error, setError] = useState<string | null>(null);
    usePoll({
        every: 30_000,
        version: panel.version,
        read: (signal) => api.helperServed(signal).then((fresh) => !signal.aborted && setServed(fresh)),
    });
    const station = stationOf(view, panel.current?.device);
    const helping = station?.record?.placement === 'stationServer';
    const reachable = station?.live.reachable ?? panel.role.station?.reachable ?? false;
    const home = station?.live.health?.placement?.homeServer;
    const without = home?.without ?? [];
    const mayManage = can('stations.manage');
    const run = (what: string, job: () => Promise<unknown>) => {
        setBusy(what);
        setError(null);
        void job()
            .then(() => reread())
            .catch((e) => setError(problemOf(e)))
            .finally(() => setBusy(null));
    };
    return (
        <div class="flex flex-col gap-10">
            <label class="w-toggle">
                <span class="flex-1 min-w-0 flex flex-col">
                    <span class="w-strong">{tr.desk.helpStation}</span>
                    <span class="w-cap">
                        {!helping
                            ? tr.desk.helpOff
                            : home?.state === 'down'
                              ? tr.desk.helpDown
                              : tr.desk.helpOn}
                    </span>
                </span>
                <Switch
                    on={helping}
                    label={tr.desk.helpStation}
                    disabled={!mayManage || busy !== null || !reachable}
                    onChange={(on) => run('all', () => api.setStationHelper(on, station?.key))}
                />
            </label>
            {/* Один переключатель; по частям — «Тонкая настройка», слой разработчика (docs/plans/panel-rebuild.md, этап 6). */}
            {!dev && helping && <p class="w-cap m-0">{tr.desk.helpAll}</p>}
            {dev && <span class="w-cap">{tr.desk.fineTuning}</span>}
            {dev && (
            <div class="w-rows">
                {parts().map((part) => {
                    const on = helping && !without.includes(part.id);
                    const today = served?.[part.id];
                    return (
                        <div key={part.id} class="w-row">
                            <span class={`w-dot ${on ? 'w-dot-own' : 'w-dot-off'}`} aria-hidden="true" />
                            <span class="w-text">
                                {part.title}
                                <span class="w-cap">
                                    {' · '}
                                    {on
                                        ? today && today.count > 0
                                            ? `${part.served(today.count)} · ${seconds(today.avgMs)}`
                                            : tr.desk.here
                                        : tr.desk.nowWithout(part.without)}
                                </span>
                            </span>
                            <span class="w-cap">{tr.desk.otherwise(part.without)}</span>
                            <Switch
                                on={on}
                                label={tr.desk.giveToStation(part.title.toLowerCase())}
                                disabled={!mayManage || busy !== null || !reachable || !helping}
                                onChange={(next) =>
                                    run(part.id, () =>
                                        api.setHelperParts(next ? without.filter((id) => id !== part.id) : [...without, part.id], station?.key),
                                    )
                                }
                            />
                        </div>
                    );
                })}
            </div>
            )}
            {dev && served && <p class="w-cap m-0">{tr.desk.countToday}</p>}
            <KeepAwake />
            {error && (
                <p class="w-cap c-dot-offline m-0" role="alert">
                    {error}
                </p>
            )}
        </div>
    );
}

/**
 * «Не давать компьютеру спать» (макет Main): уснёт — колонка не замолчит (слышит облако, говорит Алиса), но «Свой»
 * вернётся, только когда помощник проснётся. Настройка приложения DuoWave: в браузере вне его её нет.
 */
function KeepAwake() {
    const [state, setState] = useState<{ managed: boolean; keepAwake?: boolean } | null>(null);
    const [busy, setBusy] = useState(false);
    useEffect(() => {
        let alive = true;
        void api
            .desktopAwake()
            .then((fresh) => alive && setState(fresh))
            .catch(() => alive && setState({ managed: false }));
        return () => {
            alive = false;
        };
    }, []);
    if (!state?.managed) return null;
    return (
        <label class="w-toggle">
            <span class="flex-1 min-w-0 flex flex-col">
                <span class="w-strong">{tr.desk.keepAwake}</span>
                <span class="w-cap">{tr.desk.keepAwakeNote}</span>
            </span>
            <Switch
                on={state.keepAwake !== false}
                label={tr.desk.keepAwake}
                disabled={busy || !can('settings.edit')}
                onChange={(on) => {
                    setBusy(true);
                    void api
                        .setDesktopAwake(on)
                        .then(setState)
                        .catch(() => undefined)
                        .finally(() => setBusy(false));
                }}
            />
        </label>
    );
}

/**
 * «Присматриваю за колонкой»: копия настроек, обновления, страховка, память. «Открыть» — всё о размещении: на столе —
 * переход в «Колонку», на странице «Компьютер-помощник» (`onOpen`) — блок «Где работает DuoWave» под карточкой, ведь
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
    const last = station.backups[0];
    const memory = health?.memory;
    const mayManage = can('stations.manage');
    const updating = station.live.updating;
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
                    <span class="w-cap">{last ? `${copyDay(last.at)} ✓` : tr.desk.none}</span>
                </div>
                <div class="w-row">
                    <span class="w-text">{tr.desk.updates}</span>
                    <span class="w-cap">{station.update ? tr.desk.updateAvailable(station.update) : tr.desk.latest(station.version ?? '—')}</span>
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
            <div class="w-actions">
                {mayManage && station.live.reachable && (
                    <button class="w-btn" disabled={busy !== null} onClick={() => act('copy', () => api.backupStation(station.key), tr.desk.copyTaken)}>
                        {busy === 'copy' ? tr.desk.takingCopy : tr.desk.takeCopy}
                    </button>
                )}
                {mayManage && station.update && (
                    <button
                        class="w-btn w-btn-ink"
                        disabled={busy !== null || updating}
                        onClick={() => act('update', () => api.updateStationGateway(station.key))}
                    >
                        {busy === 'update' || updating ? tr.desk.updating : tr.desk.updateTo(station.update)}
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

/** У шлюза на колонке — его помощник: на связи ли он и что даёт; «Открыть помощника» — пульт компьютера. */
export function HelperCardWidget() {
    const panel = usePanel();
    const home = panel.health?.placement?.homeServer;
    if (!home?.url) return <p class="w-cap m-0">{tr.desk.helperNotHelping}</p>;
    let panelUrl: string | null = null;
    try {
        panelUrl = `${new URL(home.url).origin}/panel`;
    } catch {
        panelUrl = null;
    }
    const up = home.state === 'up' && home.enabled !== false;
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
            {panelUrl && (
                <div class="w-actions">
                    <a class="w-btn" href={panelUrl} target="_blank" rel="noreferrer">
                        {tr.desk.openComputerPanel}
                    </a>
                </div>
            )}
        </div>
    );
}

/** Подписи возможностей — для виджетов плагинов, которым нужны имена (журнал вопросов). */
export const capabilityTitle = (id: string): string => CAPABILITY_TITLES[id] ?? id;
