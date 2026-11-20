import { useEffect, useRef, useState } from 'preact/hooks';

import { HomeServerStatus, MoveJob, MoveTo, PlacementHealth, PlacementView, Station, StationGatewayView, api, can, problemOf } from '../api';
import { FeatureRow, GROUP_TITLES, PLACEMENT_TITLES, Placement, useAvailability } from '../availability';
import { locale, tr } from '../i18n';
import { IconLinks } from '../icons';
import { usePanel } from '../shell/context';
import { helperOf } from '../shell/role';
import { stationUpdateStep } from '../station-update';
import { timeAgo } from '../time';
import { Host, Solved, solve } from '../truth';
import { DashData, ourBrain, scenarioOf } from './dash-model';
import { Gear, NodeIcon, PlacementMap, WhoThree } from './placement-map';
import { Action } from '../ui/cards';
import { SecretField, TextField } from '../ui/fields';
import { Sheet, WinRow, WinSection } from '../ui/sheet';
import { Switch } from '../ui/switch';
import { ask } from '../ui/confirm';

/**
 * «Где работает шлюз» (docs/plans/station-placement.md): на компьютере, на колонке или на колонке с компьютером как
 * домашним сервером. Пульт компьютера показывает шлюз на колонке, каким его видит компьютер, и переносит шлюз; пульт
 * колонки — себя, свой домашний сервер и путь назад к Яндексу без компьютера. Что где доступно — список шлюза.
 */

const LEVEL_MARK: Record<FeatureRow['pc']['level'], string> = { yes: '✓', partial: '◐', no: '—' };

function mb(kb?: number): string {
    return kb === undefined ? '—' : tr.placement.mb(Math.round(kb / 1024));
}

/** «держит 10 МБ памяти, свободно 61 МБ · в /data свободно 40 МБ» — только то, что известно. */
function resources(memory?: { rssKb?: number; availableKb?: number }, data?: { freeKb: number }): string {
    const parts = [];
    if (memory?.rssKb !== undefined) parts.push(tr.placement.holdsMemory(mb(memory.rssKb)));
    if (memory?.availableKb !== undefined) parts.push(tr.placement.freeMemory(mb(memory.availableKb)));
    if (data) parts.push(tr.placement.freeData(mb(data.freeKb)));
    return parts.join(', ');
}

function clock(at?: string): string {
    if (!at) return '';
    const date = new Date(at);
    const time = date.toLocaleTimeString(locale(), { hour: '2-digit', minute: '2-digit' });
    return date.toDateString() === new Date().toDateString() ? time : `${date.toLocaleDateString(locale())} ${time}`;
}

function Line({ children, tone }: { children: preact.ComponentChildren; tone?: 'warn' | 'error' | 'ok' }) {
    const color = tone === 'warn' ? ' c-pill-yandex-fg' : tone === 'error' ? ' c-dot-offline' : tone === 'ok' ? ' c-accent-text' : '';
    return (
        <p class={`t-caption m-0${color}`} role={tone === 'error' ? 'alert' : tone ? 'status' : undefined}>
            {children}
        </p>
    );
}

function Row({ children }: { children: preact.ComponentChildren }) {
    return <div class="flex gap-8 flex-wrap items-center">{children}</div>;
}

/** Первая карточка «Дом» → «Шлюз». */
export function PlacementCard() {
    const [view, setView] = useState<PlacementView | null>(null);
    const [message, setMessage] = useState<{ ok: boolean; text: string } | null>(null);
    const [busy, setBusy] = useState<string | null>(null);
    const [choosing, setChoosing] = useState(false);
    const [inspected, setInspected] = useState<Inspected | null>(null);
    const [job, setJob] = useState<MoveJob | null>(null);
    const [linking, setLinking] = useState(false);
    const [unlinking, setUnlinking] = useState(false);
    const [givingBack, setGivingBack] = useState(false);
    const alive = useRef(true);

    const read = () =>
        api
            .placement()
            .then((answer) => {
                if (!alive.current) return;
                setView(answer);
                if (answer.job) setJob(answer.job);
            })
            .catch(() => undefined);

    useEffect(() => {
        alive.current = true;
        void read();
        const timer = window.setInterval(() => void read(), 15_000);
        return () => {
            alive.current = false;
            window.clearInterval(timer);
        };
    }, []);

    // Пока идёт переход — его шаги чаще.
    useEffect(() => {
        if (!job || job.status !== 'running') return;
        const timer = window.setInterval(() => {
            api.moveJob(job.id)
                .then((next) => {
                    setJob(next);
                    if (next.status !== 'running') void read();
                })
                .catch(() => undefined);
        }, 1500);
        return () => window.clearInterval(timer);
    }, [job?.id, job?.status]);

    if (!view) return null;

    const act = async (name: string, work: () => Promise<unknown>, done?: string) => {
        setBusy(name);
        setMessage(null);
        try {
            await work();
            if (done) setMessage({ ok: true, text: done });
            await read();
        } catch (e) {
            setMessage({ ok: false, text: problemOf(e) });
        } finally {
            setBusy(null);
        }
    };

    const mayManage = can('stations.manage');
    // Листы — рядом с карточкой, не внутри: у стекла backdrop-filter, и fixed внутри него держится карточки.
    return (
        <>
            <section class="card glass px-16 pt-4 pb-14 flex flex-col gap-10" aria-labelledby="placement-h">
                <div class="section-head">
                    <h2 id="placement-h" class="t-title-xs m-0">
                        {tr.placement.gatewayHead}
                    </h2>
                    <span class="pill pill-own">{PLACEMENT_TITLES[view.role === 'pc' ? (view.station?.record?.placement ?? 'pc') : view.placement]}</span>
                </div>
                {view.role === 'station' ? (
                    <StationSide view={view} busy={busy} act={act} mayManage={mayManage} onGiveBack={() => setGivingBack(true)} />
                ) : (
                    <ComputerSide
                        view={view}
                        busy={busy}
                        act={act}
                        mayManage={mayManage}
                        onChoose={() => setChoosing(true)}
                        onInspect={() =>
                            void act('inspect', async () => {
                                setInspected(await api.inspectStation());
                            })
                        }
                        onLink={() => setLinking(true)}
                        onUnlink={() => setUnlinking(true)}
                    />
                )}
                {job && <MoveProgress job={job} />}
                {message && <Line tone={message.ok ? 'ok' : 'error'}>{message.text}</Line>}
            </section>
            {linking && (
                <LinkSheet
                    onClose={() => setLinking(false)}
                    onLinked={() => {
                        setLinking(false);
                        void read();
                    }}
                />
            )}
            {choosing && (
                <PlacementChooser
                    view={view}
                    onClose={() => setChoosing(false)}
                    onStarted={(started) => {
                        setJob(started);
                        setChoosing(false);
                    }}
                />
            )}
            {inspected && <InspectSheet inspected={inspected} onClose={() => setInspected(null)} />}
            {givingBack && (
                <GiveBackSheet
                    onClose={() => setGivingBack(false)}
                    onConfirm={async () => {
                        await act('giveBack', () => api.giveBack(), tr.placement.giveBackDone);
                        setGivingBack(false);
                    }}
                />
            )}
            {unlinking && view.station?.record && (
                <UnlinkSheet
                    onClose={() => setUnlinking(false)}
                    onConfirm={async () => {
                        const address = view.station!.record!.address;
                        await act('yandex', async () => {
                            setJob(await api.startMove({ to: 'yandex', address }));
                        });
                        setUnlinking(false);
                    }}
                />
            )}
        </>
    );
}

type Act = (name: string, work: () => Promise<unknown>, done?: string) => Promise<void>;

/** «Что случилось»: колонка о своём шлюзе — по её оболочке, даже когда сломана сама программа. */
type Inspected = Awaited<ReturnType<typeof api.inspectStation>>;

/**
 * «Разрешить в брандмауэре»: колонка не достучалась до этого компьютера, а он — Windows (тест 21.11: брандмауэр
 * закрыл компьютер, «Турбо нет» видела только колонка). Одно разрешение для частной сети; Windows спросит права.
 */
function AllowFirewall({ act, busy }: { act: Act; busy: string | null }) {
    const [windows, setWindows] = useState(false);
    useEffect(() => {
        let alive = true;
        api.windowsNetwork()
            .then((view) => alive && setWindows(view.windows))
            .catch(() => undefined);
        return () => {
            alive = false;
        };
    }, []);
    if (!windows) return null;
    return (
        <Row>
            <Action
                label={tr.placement.allowFirewall}
                busy={busy === 'firewall'}
                onClick={() => void act('firewall', () => api.allowWindowsFirewall(), tr.placement.allowFirewallDone)}
            />
        </Row>
    );
}

/** Пульт компьютера: шлюз на колонке, каким его видит компьютер. */
function ComputerSide({
    view,
    busy,
    act,
    mayManage,
    onChoose,
    onInspect,
    onLink,
    onUnlink,
}: {
    view: PlacementView;
    busy: string | null;
    act: Act;
    mayManage: boolean;
    onChoose: () => void;
    onInspect: () => void;
    onLink: () => void;
    onUnlink: () => void;
}) {
    const station = view.station;
    const record = station?.record;
    const onStation = record && record.placement !== 'pc';
    if (!onStation) {
        return (
            <>
                <Line>
                    {tr.placement.runsOnThisComputer}
                </Line>
                {station?.payload.source === 'none' && <Line tone="warn">{station.payload.note}</Line>}
                {mayManage && (
                    <Row>
                        <Action label={tr.placement.change} onClick={onChoose} />
                        <Action label={tr.placement.gatewayAlreadyOnStation} onClick={onLink} />
                    </Row>
                )}
            </>
        );
    }
    const live = station.live;
    const health = live.health?.placement;
    const memory = health?.memory;
    const step = stationUpdateStep(station);
    const homeServer = health?.homeServer;
    return (
        <>
            <Line>
                {live.reachable
                    ? [tr.placement.gatewayOnStationOnline(station.version ?? ''), resources(memory, health?.data)].filter(Boolean).join(' · ')
                    : live.seenAt
                      ? tr.placement.gatewayOnStationDown(timeAgo(live.seenAt))
                      : tr.placement.gatewayOnStationSilent}
            </Line>
            <label class="helper-row">
                <span class="helper-icon">
                    <SegIcon placement="pc" />
                </span>
                <span class="helper-text">
                    {tr.placement.pcHelps}
                    <span>
                        {record.placement !== 'stationServer'
                            ? tr.placement.helperOff
                            : homeServer?.state === 'down'
                              ? tr.placement.helperDown
                              : tr.placement.helperOn}
                    </span>
                </span>
                <Switch
                    on={record.placement === 'stationServer'}
                    label={tr.placement.pcHelps}
                    disabled={!mayManage || busy === 'helper' || !live.reachable}
                    onChange={(on) =>
                        void act('helper', () => api.setStationHelper(on), on ? tr.placement.helperSwitchOn : tr.placement.helperSwitchOff)
                    }
                />
            </label>
            {!station.compatible && <Line tone="warn">{tr.placement.apiMismatch}</Line>}
            {station.warnings.map((warning) => (
                <div key={warning.kind} class="flex flex-col gap-6">
                    <Line tone="warn">{warning.text}</Line>
                    {mayManage && warning.kind === 'silent' && (
                        <Row>
                            <Action label={tr.placement.check} busy={busy === 'inspect'} onClick={onInspect} />
                            <Action
                                label={tr.placement.retry}
                                busy={busy === 'retry'}
                                onClick={() => void act('retry', () => api.retryStation(), tr.placement.retried)}
                            />
                        </Row>
                    )}
                    {mayManage && warning.kind === 'unreached' && <AllowFirewall act={act} busy={busy} />}
                    {mayManage && warning.kind === 'voiceAway' && (
                        <Row>
                            <Action
                                label={tr.placement.giveVoiceBack}
                                busy={busy === 'voice'}
                                onClick={() =>
                                    void act('voice', () => api.stationVoice('local'), tr.placement.voiceBackDone)
                                }
                            />
                        </Row>
                    )}
                </div>
            ))}
            <Row>
                <a class="pill pill-off py-8 px-14" href={`http://${record.address}:${record.port}/panel`} target="_blank" rel="noreferrer">
                    {tr.placement.stationPanel}
                </a>
                {mayManage && <Action label={tr.placement.change} onClick={onChoose} />}
            </Row>
            <SettingsLine view={view} />
            <div class="flex flex-col gap-6">
                {/* Что сделает «Обновить колонку»: компьютер ставит ей только свою версию, выпуск новее него — сначала на
                    него (owner 20.11). «Последняя версия» — только когда нового нет нигде. */}
                {step?.kind === 'send' || step?.kind === 'computerFirst' ? (
                    <Row>
                        <Line>{step.kind === 'send' ? tr.placement.newerGateway(step.version) : tr.placement.newerRelease(step.version)}</Line>
                        {can('system.update') && (
                            <Action
                                tone="on"
                                label={
                                    live.updating
                                        ? tr.placement.updating
                                        : step.kind === 'send'
                                          ? tr.placement.updateTo(step.version)
                                          : tr.placement.updateBoth(step.version)
                                }
                                busy={live.updating || busy === 'update'}
                                onClick={() => void act('update', () => api.updateStationGateway(station.key))}
                            />
                        )}
                    </Row>
                ) : step?.kind === 'blocked' ? (
                    <Line>{step.why}</Line>
                ) : step?.kind === 'stationAhead' ? (
                    <Line>{tr.placement.stationAhead(step.station, step.computer)}</Line>
                ) : step?.kind === 'follows' ? (
                    <Line>{tr.placement.followsComputer(step.version)}</Line>
                ) : (
                    live.reachable && <Line>{tr.placement.latestGateway}</Line>
                )}
                {live.updateError && <Line tone="error">{tr.placement.updateFailed(live.updateError)}</Line>}
                {health?.update?.trial && (
                    <Line>
                        {tr.placement.trialOnStation(health.update.trial.version, clock(health.update.trial.until))}
                        {health.update.trial.appsPaused && ` ${tr.placement.trialAppsPaused}`}
                    </Line>
                )}
                {!health?.update?.trial && health?.update?.trialFailed && (
                    <Line tone="warn">
                        {health.update.trialFailed.why === 'rebooted'
                            ? tr.placement.trialRebooted(health.update.trialFailed.version)
                            : tr.placement.trialCrashed(health.update.trialFailed.version)}
                    </Line>
                )}
                {can('system.update') && record.managed && (
                    <label class="t-caption flex items-center gap-10">
                        <Switch
                            on={record.autoUpdate}
                            label={tr.placement.autoUpdate}
                            disabled={busy === 'auto'}
                            onChange={(on) => void act('auto', () => api.setStationAutoUpdate(on))}
                        />
                        {tr.placement.autoUpdateAbout}
                    </label>
                )}
                {mayManage && health && (
                    <label class="t-caption flex items-center gap-10">
                        <Switch
                            on={health.safety !== false}
                            label={tr.placement.safety}
                            disabled={busy === 'safety' || !live.reachable}
                            onChange={(on) => void act('safety', () => api.setSafety(on))}
                        />
                        {tr.placement.safetyAbout}
                    </label>
                )}
            </div>
            {mayManage && (
                <button class="danger-link" disabled={busy === 'yandex'} onClick={onUnlink}>
                    {tr.placement.unlink}
                </button>
            )}
        </>
    );
}

/**
 * Настройки колонки и их копии (docs/plans/desk.md, этап 4): настройки живут на колонке, помощник правит их там, в её
 * окнах, а здесь держит копии. Выбора «на колонке / на компьютере» больше нет; колонка, чьи настройки пока на
 * компьютере (так было прежде), возвращает их на колонку одной кнопкой.
 */
function SettingsLine({ view }: { view: PlacementView }) {
    const station = view.station!;
    const live = station.live;
    // The home's copy (docs/plans/home-and-devices.md, stage 3): this computer takes it at each change of the centre.
    const copied = station.copiedAt;
    const text = station.copyStale
        ? copied
            ? tr.placement.settingsStale(clock(copied))
            : tr.placement.settingsNoCopy
        : tr.placement.settingsFresh(copied ? timeAgo(copied) : '—');
    return (
        <div class="flex flex-col gap-6">
            <Line tone={station.copyStale ? 'warn' : undefined}>{text}</Line>
            {live.syncError && <Line tone="error">{live.syncError}</Line>}
        </div>
    );
}

/** Пульт колонки: шлюз на ней, её домашний сервер, путь назад к Яндексу без компьютера. */
function StationSide({
    view,
    busy,
    act,
    mayManage,
    onGiveBack,
}: {
    view: PlacementView;
    busy: string | null;
    act: Act;
    mayManage: boolean;
    onGiveBack: () => void;
}) {
    const health = view.health;
    const voiceHere = health.voice?.local !== false;
    const managed =
        view.managedBy === 'pc'
            ? tr.placement.managedPc
            : view.managedBy === 'yam'
              ? tr.placement.managedYam
              : view.managedBy === 'yse'
                ? tr.placement.managedYse
                : tr.placement.managedFeed;
    return (
        <>
            <Line>{[tr.placement.runsOnStation, resources(health.memory, health.data)].filter(Boolean).join(' · ')}</Line>
            {/* Шлюз со списком помощников (docs/plans/home-and-helpers.md, блок 1): одна строка — всё о них в «Помощниках»;
                колонка 0.11 — прежний переключатель «ПК помогает колонке». */}
            {health.helpers ? (
                <HelpersLine health={health} />
            ) : (
                <StationHelper mayManage={can('connectors.manage')} onChanged={() => void act('helperRead', async () => undefined)} />
            )}
            <Line>
                {health.settingsCopiedAt ? tr.placement.settingsCopiedAt(timeAgo(health.settingsCopiedAt)) : tr.placement.settingsNotCopied}
            </Line>
            <Line>{managed}</Line>
            {view.state.safetyLast && (
                <Line tone="warn">
                    {tr.placement.safetyFired(clock(view.state.safetyLast.at))}
                    {view.state.safetyLast.what === 'previous-version' ? tr.placement.safetyPrevious : tr.placement.safetyAddress}
                </Line>
            )}
            {mayManage && (
                <label class="t-caption flex items-center gap-10">
                    <Switch
                        on={view.state.safety}
                        label={tr.placement.safety}
                        disabled={busy === 'safety'}
                        onChange={(on) => void act('safety', () => api.setSafety(on))}
                    />
                    {tr.placement.safetyAbout}
                </label>
            )}
            {mayManage && (
                <Row>
                    {voiceHere ? (
                        <button class="danger-link soft" disabled={busy === 'giveBack'} onClick={onGiveBack}>
                            {tr.placement.giveVoice}
                        </button>
                    ) : (
                        <>
                            <Line tone="warn">{tr.placement.voiceNotHere}</Line>
                            <Action
                                label={tr.placement.giveVoiceBack}
                                busy={busy === 'pointLocal'}
                                onClick={() =>
                                    void act('pointLocal', () => api.pointLocal(), tr.placement.voiceBackDone)
                                }
                            />
                        </>
                    )}
                </Row>
            )}
        </>
    );
}

/** Шаги перехода. */
export function MoveProgress({ job }: { job: MoveJob }) {
    return (
        <div role="status" class="flex flex-col gap-4 py-10 px-12 r-panel bg-divider">
            <span class="fw-700 fs-13">
                {tr.placement.moveTitles[job.to]}
                {job.status === 'done' ? tr.placement.moveDone : job.status === 'failed' ? tr.placement.moveFailed : '…'}
            </span>
            {job.steps.map((step) => (
                <span key={step.id} class={`t-caption flex gap-8${step.state === 'pending' || step.state === 'skipped' ? ' opacity-50' : ''}`}>
                    <span aria-hidden="true" class="w-14">
                        {step.state === 'done' ? '✓' : step.state === 'failed' ? '✕' : step.state === 'running' ? '…' : step.state === 'skipped' ? '–' : '·'}
                    </span>
                    {tr.placement.stepTitles[step.id]}
                </span>
            ))}
            {job.note && <Line>{job.note}</Line>}
            {job.error && <Line tone="error">{job.error}</Line>}
        </div>
    );
}

/** Таблица «Что доступно» для одного размещения: что есть, что частично, чего нет — и почему. */
function FeatureList({ rows, placement }: { rows: FeatureRow[]; placement: Placement }) {
    const groups = [...new Set(rows.map((row) => row.group))];
    return (
        <div class="flex flex-col gap-8">
            {groups.map((group) => (
                <div key={group} class="flex flex-col gap-2">
                    <span class="t-meta fw-700">
                        {GROUP_TITLES[group] ?? group}
                    </span>
                    {rows
                        .filter((row) => row.group === group)
                        .map((row) => {
                            const cell = row[placement];
                            return (
                                <span key={row.id} class={`t-caption flex gap-8${cell.level === 'no' ? ' opacity-55' : ''}`}>
                                    <span aria-label={cell.level === 'yes' ? tr.placement.levelYes : cell.level === 'partial' ? tr.placement.levelPartial : tr.placement.levelNo} class="w-14 flex-none">
                                        {LEVEL_MARK[cell.level]}
                                    </span>
                                    <span class="flex-1">
                                        {row.title}
                                        {cell.note !== 'да' && cell.note !== 'нет' && <span class="t-meta"> — {cell.note}</span>}
                                    </span>
                                </span>
                            );
                        })}
                </div>
            ))}
        </div>
    );
}

/** Что будет при каждом размещении — коротко, по четыре строки: переключение сегментов не двигает страницу. */
const WHAT_KINDS: Record<Placement, Array<'plus' | 'minus' | 'info'>> = {
    pc: ['plus', 'minus', 'info', 'info'],
    station: ['plus', 'minus', 'minus', 'info'],
    stationServer: ['plus', 'plus', 'minus', 'info'],
};

const segTitle = (placement: Placement): string => ({ pc: tr.placement.segPc, station: tr.placement.segStation, stationServer: tr.placement.segStationServer })[placement];

function SegIcon({ placement }: { placement: Placement }) {
    return (
        <svg width="22" height="22" viewBox="0 0 24 24" aria-hidden="true">
            {placement === 'stationServer' ? (
                <>
                    <g transform="translate(-3.5 3) scale(0.75)">
                        <NodeIcon host="station" />
                    </g>
                    <g transform="translate(9.5 3) scale(0.65)">
                        <NodeIcon host="pc" />
                    </g>
                </>
            ) : (
                <NodeIcon host={placement === 'pc' ? 'pc' : 'station'} />
            )}
        </svg>
    );
}

/** «Сменить…»: где работает DuoWave — три варианта, схема, что будет, связь без адресов (решения 2, 6, 7). */
function PlacementChooser({ view, onClose, onStarted }: { view: PlacementView; onClose: () => void; onStarted: (job: MoveJob) => void }) {
    const availability = useAvailability();
    const record = view.station?.record;
    const current: Placement = record?.placement ?? 'pc';
    const [picked, setPicked] = useState<Placement>(current === 'pc' ? 'stationServer' : current);
    const [stations, setStations] = useState<Station[] | null>(null);
    const [address, setAddress] = useState(record?.address ?? '');
    const [gateway, setGateway] = useState('');
    const [editing, setEditing] = useState<{ station: boolean; pc: boolean }>({ station: false, pc: false });
    // Настройки колонки живут на ней (docs/plans/desk.md, этап 4): перенос на колонку их туда и кладёт.
    const home = 'station' as const;
    const [error, setError] = useState<string | null>(null);
    const [busy, setBusy] = useState(false);
    const [data, setData] = useState<DashData>({ mode: 'gateway', availability });

    useEffect(() => {
        void Promise.all([
            api.mode().catch(() => null),
            api.health().catch(() => null),
            can('connectors.view') ? api.connectors().catch(() => null) : null,
            can('station.view') ? api.models().catch(() => null) : null,
            can('connectors.view') ? api.brain().catch(() => null) : null,
        ]).then(([mode, health, connectors, models, brain]) =>
            setData({
                mode: mode?.mode ?? health?.panelMode ?? 'gateway',
                routes: health?.skills?.routes,
                brain: brain?.status ?? health?.brain,
                brainView: brain,
                models,
                connectors: connectors?.connectors ?? null,
                availability,
                yandexEar: mode?.yandexEar ?? health?.yandexEar,
            }),
        );
        void api
            .stations()
            .then((list) => {
                setStations(list);
                if (!address) {
                    const known = list.find((entry) => entry.status === 'approved' && entry.lastAddress);
                    if (known?.lastAddress) setAddress(known.lastAddress);
                }
            })
            .catch(() => setStations([]));
    }, []);

    const to: MoveTo = picked;
    const same = picked === current;
    const onStation = picked !== 'pc';
    // Сборки для колонки здесь нет и взять её неоткуда: перенести на колонку нечего.
    const noBuild = onStation && view.station?.payload.source === 'none';
    const full = { ...data, availability };
    // Окно объясняет, как может работать, а не только как сейчас: облако в схеме считается подключённым даже без
    // ключей. Настоящее состояние — рядом припиской, когда облачного распознавания или голоса на самом деле нет.
    const real = scenarioOf(full, picked);
    const solved = solve({ ...real, cloudEar: true, cloudVoice: true });
    const brain = real.brain;
    const cloudAssumed = !real.cloudEar || !real.cloudVoice;
    // Имя выбранной модели — на карточку «Модели и агенты»; никакой не выбрано — список примеров.
    const chosen = ourBrain(data.brain);
    const brainName = chosen.usable && chosen.tone === 'cloud' ? chosen.name : undefined;
    const station = stations?.find((entry) => entry.lastAddress === address.trim());
    const pcAddress = gateway.trim() || bestHome(view.homeAddresses ?? [], address.trim());
    const settingsAt: Host = onStation ? 'station' : 'pc';
    // Шлюз уже на колонке: «На колонке» ↔ «Колонка + ПК» — переключатель, без переноса.
    const helperOnly = current !== 'pc' && onStation && picked !== current;

    const start = async () => {
        setBusy(true);
        setError(null);
        try {
            if (helperOnly) {
                await api.setStationHelper(picked === 'stationServer');
                onClose();
                return;
            }
            const job = await api.startMove({
                to,
                address: address.trim(),
                gateway: gateway.trim() || undefined,
                settingsHome: onStation ? home : undefined,
            });
            onStarted(job);
        } catch (e) {
            setError(problemOf(e));
        } finally {
            setBusy(false);
        }
    };
    const label = busy
        ? tr.placement.starting
        : same
          ? tr.placement.already
          : helperOnly
            ? picked === 'stationServer'
                ? tr.placement.helperTurnOn
                : tr.placement.helperTurnOff
            : picked === 'pc'
              ? tr.placement.moveToPc
              : picked === 'station'
                ? tr.placement.moveToStation
                : tr.placement.moveToStationServer;
    return (
        <Sheet
            title={tr.placement.gatewayHead}
            about={helperOnly ? tr.placement.chooserAboutHelper : tr.placement.chooserAboutMove}
            onClose={onClose}
            footer={
                <button class="btn btn-main" disabled={busy || same || noBuild || !address.trim()} aria-busy={busy || undefined} onClick={() => void start()}>
                    {busy && <span class="spinner spinner-sm" aria-hidden="true" />}
                    {label}
                </button>
            }
        >
            <div role="radiogroup" aria-label={tr.placement.gatewayHead} class="place-seg">
                {(['pc', 'station', 'stationServer'] as const).map((id) => (
                    <button key={id} role="radio" aria-checked={picked === id} onClick={() => setPicked(id)}>
                        <SegIcon placement={id} />
                        {segTitle(id)}
                        <span class="now">{id === current ? tr.placement.now : ''}</span>
                    </button>
                ))}
            </div>

            <div class="settings-mini" role="note" aria-label={tr.placement.settingsOfStation}>
                <span class="set-label">
                    <Gear size={16} />
                    {tr.placement.settings}
                </span>
                <p class="meta">
                    {!onStation
                        ? tr.placement.settingsOnPcMini
                        : tr.placement.settingsOnStationMini}
                </p>
            </div>

            <PlacementMap solved={solved} placement={picked} settingsAt={settingsAt} brainName={brainName} />
            {cloudAssumed && <p class="win-note">{tr.placement.cloudAssumed}</p>}

            {onStation ? (
                <label class="helper-row">
                    <span class="helper-icon">
                        <SegIcon placement="pc" />
                    </span>
                    <span class="helper-text">
                        {tr.placement.pcHelps}
                        <span>
                            {picked === 'stationServer' ? tr.placement.helperOnDeferred : tr.placement.helperOff}
                        </span>
                    </span>
                    <Switch on={picked === 'stationServer'} label={tr.placement.pcHelps} onChange={(on) => setPicked(on ? 'stationServer' : 'station')} />
                </label>
            ) : (
                // То же место, тем же размером: в «На компьютере» помощь ПК не нужна — шлюз и есть ПК.
                <div class="helper-row" aria-hidden="true">
                    <span class="helper-icon">
                        <SegIcon placement="pc" />
                    </span>
                    <span class="helper-text">
                        {tr.placement.pcHelps}
                        <span>{tr.placement.helperNotNeeded}</span>
                    </span>
                </div>
            )}

            <WhoThree ear={solved.ear} brain={solved.brain} voice={solved.voice} />
            {solved.notes.length > 0 && <p class="win-callout m-0">{solved.notes.join(' ')}</p>}

            <WinSection title={tr.placement.whatWillBe}>
                <ul class="what-list">
                    {WHAT_KINDS[picked].map((kind, index) => (
                        <li key={index}>
                            <span class={`mk ${kind}`} aria-hidden="true">
                                {kind === 'plus' ? '+' : kind === 'minus' ? '−' : '·'}
                            </span>
                            <span>{tr.placement.what[picked][index]}</span>
                        </li>
                    ))}
                </ul>
            </WinSection>

            {!helperOnly && (
                <WinSection title={tr.placement.link} group>
                    <div class="win-row">
                        <span class={`link-dot flex-none${address.trim() ? '' : ' warn'}`} aria-hidden="true" />
                        <div class="win-row-main">
                            <span class="win-row-t">
                                {station ? tr.placement.stationNamed(station.name) : address.trim() ? tr.placement.stationPlain : tr.placement.stationNotFound}
                            </span>
                            <span class="win-row-s">
                                {address.trim() ? (
                                    station ? (
                                        tr.placement.foundInStations
                                    ) : (
                                        <span class="win-code">{address.trim()}</span>
                                    )
                                ) : (
                                    tr.placement.connectInStations
                                )}
                            </span>
                        </div>
                        <button class="btn btn-plain btn-sm" onClick={() => setEditing({ ...editing, station: !editing.station })}>
                            {editing.station ? tr.placement.done : tr.placement.edit}
                        </button>
                    </div>
                    {(editing.station || !address.trim()) && (
                        <div class="win-row is-col">
                            <TextField
                                label={tr.placement.stationAddress}
                                value={address}
                                placeholder="192.0.2.23"
                                inputMode="url"
                                hint={stations && !stations.length ? tr.placement.noStationsYet : undefined}
                                onInput={setAddress}
                            />
                        </div>
                    )}
                    <div class="win-row">
                        <span class={`link-dot flex-none${pcAddress ? '' : ' warn'}`} aria-hidden="true" />
                        <div class="win-row-main">
                            <span class="win-row-t">{tr.placement.pcThis}</span>
                            <span class="win-row-s">{pcAddress ? tr.placement.pcVisible : tr.placement.pcNotFound}</span>
                        </div>
                        <button class="btn btn-plain btn-sm" onClick={() => setEditing({ ...editing, pc: !editing.pc })}>
                            {editing.pc ? tr.placement.done : tr.placement.edit}
                        </button>
                    </div>
                    {(editing.pc || !pcAddress) && (
                        <div class="win-row is-col">
                            <TextField
                                label={tr.placement.pcAddress}
                                value={gateway}
                                placeholder={pcAddress || '192.0.2.10:3000'}
                                inputMode="url"
                                hint={view.homeAddresses?.length ? tr.placement.pcOnNetwork(view.homeAddresses.join(', ')) : undefined}
                                onInput={setGateway}
                            />
                        </div>
                    )}
                    <div class="win-row">
                        <span class="link-dot flex-none" aria-hidden="true" />
                        <div class="win-row-main">
                            <span class="win-row-t">{tr.placement.homeKey}</span>
                            <span class="win-row-s">{onStation ? tr.placement.homeKeyStation : tr.placement.homeKeyPc}</span>
                        </div>
                    </div>
                </WinSection>
            )}

            {!helperOnly &&
                (onStation ? (
                    <p class="win-callout m-0" role="status">
                        {tr.placement.keysOnStation}
                    </p>
                ) : (
                    <p class="win-note">{tr.placement.keysOnPc}</p>
                ))}
            {availability && (
                <details>
                    <summary class="win-sec-title cursor-pointer">{tr.placement.availabilityDetails}</summary>
                    <div class="pt-8 flex flex-col gap-8">
                        <FeatureList rows={availability.table} placement={picked} />
                        {view.station?.payload && onStation && <p class="win-note">{view.station.payload.note}</p>}
                    </div>
                </details>
            )}
            {error && (
                <p role="alert" class="win-callout m-0">
                    {error}
                </p>
            )}
        </Sheet>
    );
}

/** Адрес ПК в сети колонки — как выберет шлюз (watch.rs, pick_home): те же первые три числа, иначе первый. */
function bestHome(addresses: string[], station: string): string {
    const net = (address: string) => address.split(':')[0].split('.').slice(0, 3).join('.');
    return addresses.find((address) => station && net(address) === net(station)) ?? addresses[0] ?? '';
}

/**
 * «Вернуть колонку Яндексу» с ПК: шлюз уходит с колонки совсем, вернуть — только новым переносом. Своё окно: что
 * будет, как вернуться, галочка и отсчёт 5 секунд на красной кнопке (решение 13).
 */
function UnlinkSheet({ onClose, onConfirm }: { onClose: () => void; onConfirm: () => Promise<void> }) {
    const [left, setLeft] = useState(5);
    const [sure, setSure] = useState(false);
    const [busy, setBusy] = useState(false);
    useEffect(() => {
        const timer = window.setInterval(() => setLeft((value) => (value > 0 ? value - 1 : 0)), 1000);
        return () => window.clearInterval(timer);
    }, []);
    return (
        <Sheet
            title={tr.placement.unlinkTitle}
            onClose={onClose}
            footer={
                <>
                    <button class="btn btn-plain" onClick={onClose}>
                        {tr.placement.cancel}
                    </button>
                    <button
                        class="btn btn-danger-fill"
                        disabled={left > 0 || !sure || busy}
                        aria-busy={busy || undefined}
                        onClick={() => {
                            setBusy(true);
                            void onConfirm().finally(() => setBusy(false));
                        }}
                    >
                        {busy && <span class="spinner spinner-sm" aria-hidden="true" />}
                        {busy ? tr.placement.restarting : `${tr.placement.giveBackYandex}${left > 0 ? ` · ${left}` : ''}`}
                    </button>
                </>
            }
        >
            <ul class="what-list">
                <li>
                    <span class="mk minus">−</span>
                    <span>{tr.placement.unlinkGone}</span>
                </li>
                <li>
                    <span class="mk minus">−</span>
                    <span>{tr.placement.unlinkReboot}</span>
                </li>
                <li>
                    <span class="mk info">·</span>
                    <span>{tr.placement.unlinkKept}</span>
                </li>
                <li>
                    <span class="mk plus">+</span>
                    <span>{tr.placement.unlinkReturn}</span>
                </li>
            </ul>
            <label class="understand">
                <input type="checkbox" checked={sure} onChange={(event) => setSure((event.target as HTMLInputElement).checked)} />
                <span>{tr.placement.unlinkUnderstand}</span>
            </label>
        </Sheet>
    );
}

/** «Отдать голос Яндексу» на колонке: шлюз остаётся, вернуть — той же карточкой. Обычное подтверждение (решение 14). */
function GiveBackSheet({ onClose, onConfirm }: { onClose: () => void; onConfirm: () => Promise<void> }) {
    const [busy, setBusy] = useState(false);
    return (
        <Sheet
            title={tr.placement.giveTitle}
            onClose={onClose}
            footer={
                <>
                    <button class="btn btn-plain" onClick={onClose}>
                        {tr.placement.cancel}
                    </button>
                    <button
                        class="btn btn-main"
                        disabled={busy}
                        aria-busy={busy || undefined}
                        onClick={() => {
                            setBusy(true);
                            void onConfirm().finally(() => setBusy(false));
                        }}
                    >
                        {busy && <span class="spinner spinner-sm" aria-hidden="true" />}
                        {busy ? tr.placement.restarting : tr.placement.giveYandex}
                    </button>
                </>
            }
        >
            <ul class="what-list">
                <li>
                    <span class="mk minus">−</span>
                    <span>{tr.placement.giveVoiceGoes}</span>
                </li>
                <li>
                    <span class="mk info">·</span>
                    <span>{tr.placement.giveStays}</span>
                </li>
                <li>
                    <span class="mk plus">+</span>
                    <span>{tr.placement.giveReturn}</span>
                </li>
            </ul>
        </Sheet>
    );
}

/** Турбо одной строкой: кто помогает и на связи ли; «Открыть» — «Дом» в «Настройках». */
function HelpersLine({ health }: { health: PlacementHealth }) {
    const panel = usePanel();
    const helper = helperOf(health);
    // Every helper switched off (`none` with a name) is not «помощников нет»: they are listed, only off. Every one on
    // kept out by the regulator is not «не на связи»: nothing is broken.
    const sub = !helper
        ? tr.helpers.noneShort
        : helper.state === 'none'
          ? helper.name
              ? tr.helpers.statusOff(helper.name)
              : tr.helpers.noneShort
          : helper.state === 'kept'
            ? tr.helpers.statusKept(helper.name ?? '')
            : helper.state === 'up'
              ? tr.helpers.statusHelping(helper.name ?? '')
              : tr.helpers.statusAway(helper.name ?? '');
    return (
        <div class="helper-row">
            <span class="helper-icon">
                <IconLinks size={18} />
            </span>
            <span class="helper-text">
                {tr.helpers.pageTitle}
                <span>{sub}</span>
            </span>
            <Action label={tr.helpers.openHome} onClick={() => panel.go('settings', 'helpers')} />
        </div>
    );
}

/**
 * «ПК помогает колонке» в пульте колонки 0.11: один переключатель — адрес и ключ привёз перенос (решение 7). Адрес —
 * только под «Изменить», когда ПК не спит, а связи нет: обычно ПК сам сообщает свой новый адрес (решение 9).
 */
function StationHelper({ mayManage, onChanged }: { mayManage: boolean; onChanged: () => void }) {
    const [status, setStatus] = useState<HomeServerStatus | null>(null);
    const [editing, setEditing] = useState(false);
    const [url, setUrl] = useState('');
    const [key, setKey] = useState('');
    const [busy, setBusy] = useState(false);
    const [message, setMessage] = useState<{ ok: boolean; text: string } | null>(null);
    useEffect(() => {
        void api
            .homeServer()
            .then((answer) => {
                setStatus(answer);
                setUrl(answer.url ?? '');
            })
            .catch(() => setStatus(null));
    }, []);
    if (!status) return null;
    const enabled = status.enabled ?? status.configured;
    const run = async (body: { url?: string; key?: string; enabled?: boolean }) => {
        setBusy(true);
        setMessage(null);
        try {
            const next = await api.linkHomeServer(body);
            setStatus(next);
            setUrl(next.url ?? '');
            setKey('');
            if (body.url || body.key) setEditing(false);
            const on = next.enabled ?? next.configured;
            setMessage(
                !on
                    ? { ok: true, text: tr.placement.helperNoComputer }
                    : next.state === 'up'
                      ? { ok: true, text: tr.placement.helperUp }
                      : { ok: false, text: next.error ?? tr.placement.helperUnreachable },
            );
            onChanged();
        } catch (e) {
            setMessage({ ok: false, text: problemOf(e) });
        } finally {
            setBusy(false);
        }
    };
    const sub = !status.configured
        ? tr.placement.subUnknown
        : !enabled
          ? tr.placement.subDisabled
          : status.state === 'up'
            ? tr.placement.subUp
            : tr.placement.subDown(status.since ? clock(status.since) : '');
    const away = enabled && status.state !== 'up';
    return (
        <div class="flex flex-col gap-8">
            <label class="helper-row">
                <span class="helper-icon">
                    <SegIcon placement="pc" />
                </span>
                <span class="helper-text">
                    {tr.placement.pcHelps}
                    <span>{sub}</span>
                </span>
                <Switch
                    on={enabled}
                    label={tr.placement.pcHelps}
                    disabled={!mayManage || busy || !status.configured}
                    onChange={(on) => void run({ enabled: on })}
                />
            </label>
            {away && (
                <p class="truth-note">
                    {tr.placement.awayNote}
                </p>
            )}
            {mayManage && status.configured && (away || editing) && (
                <Row>
                    {status.url && (
                        <a class="pill pill-off py-8 px-14" href={`${status.url}/panel`} target="_blank" rel="noreferrer">
                            {tr.placement.pcPanel}
                        </a>
                    )}
                    <Action label={editing ? tr.placement.hide : tr.placement.editAddress} onClick={() => setEditing(!editing)} />
                </Row>
            )}
            {mayManage && editing && (
                <div class="flex flex-col gap-8">
                    <TextField label={tr.placement.pcAddressLabel} value={url} placeholder="http://192.0.2.10:3000" inputMode="url" onInput={setUrl} />
                    <SecretField
                        label={tr.placement.pcKey}
                        mask={status.key}
                        value={key}
                        removing={false}
                        hint={tr.placement.pcKeyHint}
                        onInput={setKey}
                        onRemove={() => undefined}
                    />
                    <Row>
                        <Action tone="on" label={tr.placement.save} busy={busy} onClick={() => void run({ url: url.trim(), key: key.trim() || undefined })} />
                    </Row>
                </div>
            )}
            {message && <Line tone={message.ok ? 'ok' : 'error'}>{message.text}</Line>}
        </div>
    );
}

/** Шлюз на колонке поставлен вручную (пакет YAM, `duowave station install`): адрес и ключ пары из его .env. */
/**
 * «Шлюз уже на колонке»: компьютер становится её помощником. По паролю пульта колонки — ключ пары она отдаёт сама
 * (docs/plans/desk.md, этап 4); ключ пары руками — запасной путь для шлюза, поставленного вручную со своим PEER_KEY.
 */
export function LinkSheet({ onClose, onLinked, initialAddress }: { onClose: () => void; onLinked: () => void; initialAddress?: string }) {
    const [address, setAddress] = useState(initialAddress ?? '');
    const [port, setPort] = useState('8790');
    const [byKey, setByKey] = useState(false);
    const [password, setPassword] = useState('');
    const [name, setName] = useState('');
    const [key, setKey] = useState('');
    const [busy, setBusy] = useState(false);
    const [error, setError] = useState<string | null>(null);
    const link = async () => {
        setBusy(true);
        setError(null);
        try {
            if (byKey) await api.linkStation(address.trim(), Number(port) || 8790, key.trim());
            else await api.pairStation(address.trim(), Number(port) || 8790, password, name.trim());
            onLinked();
        } catch (e) {
            setError(problemOf(e));
        } finally {
            setBusy(false);
        }
    };
    const ready = address.trim() && (byKey ? key.trim().length >= 16 : password.length > 0);
    return (
        <Sheet
            title={tr.placement.linkTitle}
            onClose={onClose}
            footer={
                <button class="btn btn-main" disabled={busy || !ready} aria-busy={busy || undefined} onClick={() => void link()}>
                    {busy && <span class="spinner spinner-sm" aria-hidden="true" />}
                    {busy ? tr.placement.linking : tr.placement.becomeHelper}
                </button>
            }
        >
            <p class="win-lead">{tr.placement.linkAbout}</p>
            <TextField label={tr.placement.stationAddress} value={address} placeholder="192.0.2.23" inputMode="url" onInput={setAddress} />
            {byKey ? (
                <>
                    <TextField label={tr.placement.gatewayPort} value={port} placeholder="8790" inputMode="numeric" onInput={setPort} />
                    <SecretField
                        label={tr.placement.pairKey}
                        value={key}
                        removing={false}
                        hint={tr.placement.pairKeyHint}
                        onInput={setKey}
                        onRemove={() => undefined}
                    />
                </>
            ) : (
                <>
                    <TextField label={tr.placement.nameInPanel} value={name} placeholder={tr.placement.nameHint} onInput={setName} />
                    <SecretField
                        label={tr.placement.panelPassword}
                        value={password}
                        removing={false}
                        hint={tr.placement.panelPasswordHint}
                        onInput={setPassword}
                        onRemove={() => undefined}
                    />
                </>
            )}
            <button class="btn btn-quiet btn-sm self-start" onClick={() => setByKey(!byKey)}>
                {byKey ? tr.placement.linkByPassword : tr.placement.linkByKey}
            </button>
            {error && (
                <p role="alert" class="win-callout m-0">
                    {error}
                </p>
            )}
        </Sheet>
    );
}

function InspectSheet({ inspected, onClose }: { inspected: Inspected; onClose: () => void }) {
    return (
        <Sheet title={tr.placement.inspectTitle} about={tr.placement.inspectAbout} onClose={onClose}>
            {!inspected.installed && (
                <p class="win-callout m-0" role="status">
                    {tr.placement.notInstalled}
                </p>
            )}
            {inspected.programMissing && (
                <p class="win-callout m-0" role="status">
                    {tr.placement.programMissing}
                </p>
            )}
            {inspected.installed && (
                <p class="win-lead">
                    {inspected.running
                        ? tr.placement.running
                        : inspected.disabled
                          ? tr.placement.disabledBySafety
                          : tr.placement.notRunning}{' '}
                    {inspected.local ? tr.placement.voiceGoesIn : tr.placement.voiceNotIn}
                </p>
            )}
            {inspected.fired && (
                <p class="win-callout m-0" role="status">
                    {tr.placement.fired(inspected.fired)}
                </p>
            )}
            {inspected.voiceChanges && (
                <>
                    <p class="t-meta m-0">{tr.placement.voiceChanges}</p>
                    <pre class="t-meta pre-wrap max-h-40vh overflow-auto m-0">{inspected.voiceChanges}</pre>
                </>
            )}
            {inspected.log && (
                <pre class="t-meta pre-wrap max-h-40vh overflow-auto m-0">
                    {inspected.log}
                </pre>
            )}
        </Sheet>
    );
}
