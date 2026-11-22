// «Настройки → Дом → Устройства» (docs/plans/home-and-devices.md, этап 1; холст 17.11, страница «Устройства»): все
// устройства дома одним списком — центр, колонки, что слышат для него, и Турбо, — и страница каждого, только его вещи.
// Колонка-центр: «Обзор · Комната · Устройство»; колонка, что только слышит: «Обзор · Комната»; компьютер:
// «Обзор · Этот компьютер · Другие дома · Устройство». Домашние разделы (персонаж, «Мозг», люди, подключения)
// показывают дом, здесь — только своё у устройства. Адрес страницы — `#/settings/devices/<id>?view=<вкладка>`. Под
// списком — что подключено к компьютерам дома, по видам (docs/plans/turbo-devices.md, «Пульт»).

import type { ComponentChildren, FunctionComponent } from 'preact';
import { useState } from 'preact/hooks';

import { ApiError, api, can, ownCall, problemOf } from '../api';
import { ask } from '../ui/confirm';
import type { HomeDevice, HomeDevices, PluggedKind, PluggedPlace, PluggedView } from '../api';
import { ABILITY_ICONS, abilityTitle, useHelpers } from '../helpers';
import {
    centreOf,
    deviceDot,
    deviceStateWords,
    readHomeDevices,
    useHomeDevices,
    useOwnHealth,
    usePluggedTurn,
    useThisDevice,
    useThisDeviceProbe,
} from '../home-devices';
import { usePoll } from '../poll';
import { useDevMode } from '../shell/dev';
import { viewsOf, type DeviceView } from '../home-model';
import { tr } from '../i18n';
import { IconChevronLeft, IconChevronRight, IconCloud, IconEar, IconLaptop, IconServer, IconSpeaker } from '../icons';
import { useLinkedItem, usePanel } from '../shell/context';
import { since } from '../time';
import { VIA } from '../via';
import { onComputer } from '../voice-input';
import { AddHelperSheet, CentreOverview } from './helpers';
import { HelperSide, OwnFrame, ThisComputer } from './helper-pc';
import { FoundStations } from './home-ears';
import { Stations, Updates } from './house';
import { ComputerMic, StationDevice, StationRoom } from './places';

type Icon = FunctionComponent<{ size?: number }>;

const ICONS: Record<HomeDevice['kind'], Icon> = { station: IconSpeaker, computer: IconLaptop, server: IconServer, cloud: IconCloud };

function Card({ id, title, sub, children }: { id: string; title: string; sub?: string; children: ComponentChildren }) {
    return (
        <section class="card glass px-16 pt-4 pb-14 flex flex-col gap-10" aria-labelledby={`dev-${id}`}>
            <div class="section-head flex flex-col">
                <h2 id={`dev-${id}`} class="t-title-xs m-0">
                    {title}
                </h2>
                {sub && <span class="w-cap">{sub}</span>}
            </div>
            {children}
        </section>
    );
}

/** Метки роли и «слышит» у имени устройства. */
export function DeviceMarks({ device }: { device: HomeDevice }) {
    return (
        <>
            {device.roles.includes('centre') && <span class="home-role is-centre">{tr.devices.roleCentre}</span>}
            {device.roles.includes('turbo') && <span class="home-role is-turbo">{tr.devices.roleTurbo}</span>}
            {device.hears && (
                <span class="home-hears inline-flex items-center gap-2 w-cap" title={tr.devices.hears}>
                    <IconEar size={13} />
                    {tr.devices.hears}
                </span>
            )}
        </>
    );
}

/** Строка состояния: «на связи», «спит · с 14:02», «есть копия дома». */
function stateLine(device: HomeDevice): string {
    const state = deviceStateWords(device);
    const at = device.state === 'online' ? undefined : (device.since ?? device.lastSeen);
    let words = at ? tr.devices.stateLine(state, since(at)) : state;
    // Колонка-центр: шлюз на ней отвечает, а её голос ушёл (к Яндексу) — одной строкой, не второй «колонкой».
    if (device.voice === 'offline') words = `${words} · ${tr.devices.voiceAway}`;
    else if (device.voice === 'reconnecting') words = `${words} · ${tr.devices.voiceReconnecting}`;
    return device.copy ? `${words} · ${tr.devices.copy}` : words;
}

type View = DeviceView;

const VIEW_TITLE = (): Record<View, string> => ({
    overview: tr.devices.tabOverview,
    room: tr.devices.tabRoom,
    device: tr.devices.tabDevice,
    computer: tr.devices.tabThisComputer,
    homes: tr.devices.tabOtherHomes,
});

export function DevicesPage() {
    const linked = useLinkedItem('settings', 'devices');
    const { view, older, error } = useHomeDevices();
    if (older)
        return (
            <Card id="older" title={tr.devices.pageTitle}>
                <p class="w-cap m-0">{tr.devices.older}</p>
            </Card>
        );
    if (!view)
        return (
            <p class="w-cap m-0" role={error ? 'alert' : undefined}>
                {error ?? tr.devices.reading}
            </p>
        );
    const device = linked.item ? view.devices.find((entry) => entry.id === linked.item) : undefined;
    if (device) return <DevicePage key={device.id} device={device} view={view} onBack={linked.close} />;
    return <DeviceList view={view} onOpen={(entry) => linked.open(entry.id)} />;
}

function DeviceList({ view, onOpen }: { view: HomeDevices; onOpen: (device: HomeDevice) => void }) {
    const panel = usePanel();
    const mine = useThisDevice(view, panel.health);
    const [adding, setAdding] = useState(false);
    const helpers = useHelpers().view;
    const manage = can('connectors.manage');
    return (
        <>
            <p class="w-cap m-0">{tr.devices.lead}</p>
            <Card id="list" title={view.home.name || tr.devices.homeName}>
                <div class="w-rows">
                    {view.devices.map((device) => {
                        const DeviceIcon = ICONS[device.kind];
                        return (
                            <button key={device.id} type="button" class="w-row set-row device-row" onClick={() => onOpen(device)}>
                                <span class="set-row-icon" aria-hidden="true">
                                    <DeviceIcon size={18} />
                                </span>
                                <span class="flex-1 min-w-0 flex flex-col">
                                    <span class="w-strong flex items-center gap-6 flex-wrap">
                                        {device.name}
                                        {mine?.id === device.id && <span class="w-cap">· {tr.devices.thisOne}</span>}
                                        <DeviceMarks device={device} />
                                    </span>
                                    <span class="w-cap flex items-center gap-6">
                                        <span aria-hidden="true" class={deviceDot(device)} />
                                        {stateLine(device)}
                                    </span>
                                </span>
                                <IconChevronRight size={16} />
                            </button>
                        );
                    })}
                </div>
                {view.devices.length === 1 && <p class="w-cap m-0">{tr.devices.nothingElse}</p>}
                <div class="w-actions">
                    {can('stations.manage') && !onComputer() && (
                        <button class="w-btn" onClick={panel.openConnect}>
                            {tr.devices.addStation}
                        </button>
                    )}
                    {manage && helpers && (
                        <button class="w-btn" onClick={() => setAdding(true)}>
                            {tr.devices.addTurbo}
                        </button>
                    )}
                </div>
            </Card>
            <Plugged />
            <OtherStations view={view} onJoined={(device) => onOpen(device)} />
            {/* Колонки в сети не в доме — уши одной кнопкой (этап 5 плана home-setup-flow): ищет компьютер-Турбо. */}
            <FoundStationsHere />
            {can('station.view') && !onComputer() && <Stations />}
            {adding && helpers && <AddHelperSheet listed={helpers.helpers} onClose={() => setAdding(false)} onDone={() => setAdding(false)} />}
        </>
    );
}

/** Колонки, что нашёл компьютер и которых нет в доме: в доме — те, что компьютер уже знает (центр и уши). */
function FoundStationsHere() {
    const own = useOwnHealth();
    // Только компьютер ищет колонки в сети, и только когда знает, что он компьютер (поиск читает их SSH-приветствие).
    if (!own?.placement || own.placement.role !== 'pc') return null;
    const inHome = new Set((own.placement.stations ?? []).map((station) => station.address));
    return <FoundStations inHome={inHome} onJoined={() => undefined} />;
}

/**
 * «Ещё одна колонка» (docs/plans/home-and-devices.md, этап 6): колонки, которые этот компьютер знает и на которых DuoWave
 * со своим центром, — одной кнопкой в дом ухом центра (`/api/v1/home/join-ear` у компьютера, мимо прохода), потом
 * только её комната.
 */
function OtherStations({ view, onJoined }: { view: HomeDevices; onJoined: (device: HomeDevice) => void }) {
    const own = useOwnHealth();
    const [busy, setBusy] = useState<string | null>(null);
    const [note, setNote] = useState<{ ok: boolean; text: string } | null>(null);
    const inHome = new Set(view.devices.map((device) => device.device).filter(Boolean));
    const centreDevice = view.devices.find((device) => device.id === view.centre)?.device;
    // Колонка-центр не «ещё не в доме», под каким бы ключом компьютер её ни помнил: окно колонки (`?via=`) открыто на её
    // ключе, у неё устройство центра, или тот же адрес (тест на Windows 21.11: центр предлагал подключить к дому сам себя).
    const listed = own?.placement?.stations ?? [];
    const centreHere = listed.find((station) => station.key === VIA || (!!centreDevice && station.device === centreDevice));
    const others = listed.filter(
        (station) =>
            station.placement !== 'pc' &&
            station !== centreHere &&
            station.address !== centreHere?.address &&
            !(station.device && (station.device === centreDevice || inHome.has(station.device))),
    );
    if (!can('stations.manage') || others.length === 0) return null;
    const join = async (key: string, device?: string): Promise<void> => {
        setBusy(key);
        setNote(null);
        try {
            await ownCall('POST', '/api/v1/home/join-ear', { station: key });
            setNote({ ok: true, text: tr.devices.joined });
            await readHomeDevices();
            const joined = device ? view.devices.find((entry) => entry.device === device) : undefined;
            if (joined) onJoined(joined);
        } catch (e) {
            setNote({ ok: false, text: problemOf(e) });
        } finally {
            setBusy(null);
        }
    };
    return (
        <Card id="other-stations" title={tr.devices.otherStationsTitle} sub={tr.devices.otherStationsLead}>
            <div class="w-rows">
                {others.map((station) => (
                    <div key={station.key} class="w-row">
                        <IconSpeaker size={16} />
                        <span class="flex-1 min-w-0 w-strong">{station.name ? tr.devices.stationNamed(station.name) : station.address}</span>
                        <button class="w-btn" disabled={busy !== null || !station.reachable} onClick={() => void join(station.key, station.device)}>
                            {tr.devices.joinHome}
                        </button>
                    </div>
                ))}
            </div>
            {note && (
                <p class={`w-cap m-0${note.ok ? '' : ' c-dot-offline'}`} role={note.ok ? 'status' : 'alert'}>
                    {note.text}
                </p>
            )}
        </Card>
    );
}

/** Разбудить компьютер: бужу, проснулся, не проснулся или отказ словами центра. */
type Waking = { state: 'waking' } | { state: 'woke' } | { state: 'slept' } | { state: 'failed'; why: string };

/**
 * «Подключено к компьютерам» (docs/plans/turbo-devices.md, «Пульт»; решение владельца 8): что подключено к компьютерам
 * дома — по видам, у каждой вещи её компьютер: «Keystation 49 · Mac». Спящий компьютер — серым, с «Разбудить», когда
 * центр умеет его будить. Читается с экраном, по новостям о подключённом и раз в минуту: что воткнули в Турбо, центр
 * узнаёт при опросе, без новости. Центр постарше или вход без права пользоваться приложениями — раздела нет.
 */
function Plugged() {
    const turn = usePluggedTurn();
    /** undefined — ещё читаю, null — раздела нет. */
    const [view, setView] = useState<PluggedView | null>();
    const [error, setError] = useState<string | null>(null);
    const [waking, setWaking] = useState<Record<string, Waking>>({});
    const reread = usePoll({
        every: 60_000,
        version: turn,
        read: (signal) =>
            api.homePlugged(signal).then(
                (fresh) => {
                    if (signal.aborted) return;
                    setView(fresh);
                    setError(null);
                },
                (e) => {
                    if (signal.aborted) return;
                    if (e instanceof ApiError && (e.status === 403 || e.status === 404)) setView(null);
                    else setError(problemOf(e));
                },
            ),
    });
    if (view === null) return null;
    const wake = async (place: PluggedPlace): Promise<void> => {
        const set = (next: Waking): void => setWaking((now) => ({ ...now, [place.place]: next }));
        set({ state: 'waking' });
        try {
            set({ state: (await api.wakePlace(place.place)).awake ? 'woke' : 'slept' });
        } catch (e) {
            set({ state: 'failed', why: problemOf(e) });
        }
        reread();
    };
    return (
        <Card id="plugged" title={tr.devices.pluggedTitle} sub={tr.devices.pluggedSub}>
            {!view ? (
                <p class="w-cap m-0 wrap-anywhere" role={error ? 'alert' : undefined}>
                    {error ?? tr.devices.pluggedReading}
                </p>
            ) : view.kinds.length === 0 ? (
                <p class="w-cap m-0">{tr.devices.pluggedNone}</p>
            ) : (
                view.kinds.map((kind) => (
                    <PluggedKindRows
                        key={kind.kind}
                        kind={kind}
                        wakeable={view.wakeable}
                        waking={waking}
                        onWake={(place) => void wake(place)}
                    />
                ))
            )}
            {view && error && (
                <p class="w-cap m-0 c-dot-offline wrap-anywhere" role="alert">
                    {error}
                </p>
            )}
        </Card>
    );
}

/**
 * Один вид: строка на место — что подключено там и чей это компьютер. Готов у двух мест и больше — «Делать на: Mac ·
 * ПК»: где дом делает дела вида отныне (решение 1). Какое место делает сейчас, центр не говорит: ничего не отмечено,
 * пока не выбрали здесь, а выбранное подтверждено его именем из ответа центра.
 */
function PluggedKindRows({
    kind,
    wakeable,
    waking,
    onWake,
}: {
    kind: PluggedKind;
    wakeable: string[];
    waking: Record<string, Waking>;
    onWake: (place: PluggedPlace) => void;
}) {
    const t = tr.devices;
    const ready = kind.places.filter((place) => place.state === 'ready');
    const [chosen, setChosen] = useState<{ place: string; name: string } | null>(null);
    const [busy, setBusy] = useState(false);
    const [error, setError] = useState<string | null>(null);
    // Выбранное здесь — пока оно среди готовых мест; ушло — снова ничего не отмечено.
    const picked = ready.length >= 2 && chosen && ready.some((place) => place.place === chosen.place) ? chosen : null;
    const choose = async (place: PluggedPlace): Promise<void> => {
        setBusy(true);
        setError(null);
        try {
            const answer = await api.choosePlugged(kind.kind, place.place);
            setChosen({ place: answer.place, name: answer.name });
        } catch (e) {
            setError(problemOf(e));
        } finally {
            setBusy(false);
        }
    };
    return (
        <div class="flex flex-col gap-4">
            <span class="w-strong">{kind.title}</span>
            <div class="w-rows">
                {kind.places.map((place) => (
                    <PluggedRow
                        key={place.place}
                        kind={kind}
                        place={place}
                        waking={waking[place.place]}
                        onWake={place.state === 'asleep' && wakeable.includes(place.place) ? () => onWake(place) : undefined}
                    />
                ))}
            </div>
            {ready.length >= 2 && (
                <div class="flex items-center gap-8 flex-wrap">
                    <span class="w-cap">{t.actOn}</span>
                    <div class="seg" role="group" aria-label={t.actOnLabel(kind.title)}>
                        {ready.map((place) => (
                            <button
                                key={place.place}
                                type="button"
                                aria-pressed={picked?.place === place.place}
                                disabled={busy}
                                onClick={() => void choose(place)}
                            >
                                {place.name}
                            </button>
                        ))}
                    </div>
                </div>
            )}
            {picked && (
                <p class="w-cap m-0" role="status">
                    {t.actingOn(picked.name)}
                </p>
            )}
            {error && (
                <p class="w-cap m-0 c-dot-offline wrap-anywhere" role="alert">
                    {error}
                </p>
            )}
        </div>
    );
}

/** «Keystation 49 · Mac»; не готово — серым, со словами почему; спит — «Разбудить», если центр умеет. */
function PluggedRow({ kind, place, waking, onWake }: { kind: PluggedKind; place: PluggedPlace; waking?: Waking; onWake?: () => void }) {
    const t = tr.devices;
    const ready = place.state === 'ready';
    // Экран компьютера называет себя так же, как его вид: тогда строка — просто компьютер.
    const things = place.instances.map((thing) => thing.title).filter((title) => title && title !== kind.title);
    const why =
        place.state === 'unplugged'
            ? t.pluggedUnplugged(place.name)
            : place.state === 'away'
              ? t.pluggedAway(place.name)
              : place.state === 'asleep'
                ? t.pluggedAsleep(place.name)
                : null;
    const caption = [why, place.cloud ? t.pluggedCloud : null].filter(Boolean).join(' · ');
    // Чем кончилось «Разбудить» — пока это правда: проснулся — пока не спит; не проснулся и отказ — пока спит.
    const asleep = place.state === 'asleep';
    const result =
        waking?.state === 'woke' && !asleep
            ? { ok: true, text: t.woke }
            : waking?.state === 'slept' && asleep
              ? { ok: true, text: t.notWoke }
              : waking?.state === 'failed' && asleep
                ? { ok: false, text: waking.why }
                : null;
    return (
        <div class="w-row">
            <span aria-hidden="true" class={`w-dot ${ready ? 'w-dot-own' : 'w-dot-off'}`} />
            <span class="flex-1 min-w-0 flex flex-col">
                <span class="wrap-anywhere">
                    <span class={`w-strong${ready ? '' : ' c-sub'}`}>{things.length > 0 ? things.join(', ') : place.name}</span>
                    {things.length > 0 && <span class="w-cap"> · {place.name}</span>}
                </span>
                {caption && <span class="w-cap">{caption}</span>}
                {result && (
                    <span class={`w-cap wrap-anywhere${result.ok ? '' : ' c-dot-offline'}`} role={result.ok ? 'status' : 'alert'}>
                        {result.text}
                    </span>
                )}
            </span>
            {onWake && (
                <button
                    class="w-btn flex-none"
                    disabled={waking?.state === 'waking'}
                    aria-busy={waking?.state === 'waking' || undefined}
                    onClick={onWake}
                >
                    {waking?.state === 'waking' ? t.waking : t.wake}
                </button>
            )}
        </div>
    );
}

function DevicePage({ device, view, onBack }: { device: HomeDevice; view: HomeDevices; onBack: () => void }) {
    const panel = usePanel();
    const linked = useLinkedItem('settings', 'devices');
    const mine = useThisDevice(view, panel.health)?.id === device.id;
    const views = viewsOf(device, mine);
    const wanted = linked.query.view as View | undefined;
    const dev = useDevMode();
    const probe = useThisDeviceProbe(view, panel.health);
    const shown: View = wanted && views.includes(wanted) ? wanted : 'overview';
    const pick = (next: View): void => linked.open(device.id, next === 'overview' ? {} : { view: next });
    const DeviceIcon = ICONS[device.kind];
    return (
        <>
            <button class="set-back is-shown" onClick={onBack}>
                <IconChevronLeft size={16} />
                {tr.devices.allDevices}
            </button>
            <div class="flex items-center gap-10">
                <span class="set-row-icon" aria-hidden="true">
                    <DeviceIcon size={20} />
                </span>
                <span class="flex-1 min-w-0 flex flex-col">
                    <span class="t-title-xs flex items-center gap-6 flex-wrap">
                        {device.name}
                        <DeviceMarks device={device} />
                    </span>
                    <span class="w-cap flex items-center gap-6">
                        <span aria-hidden="true" class={deviceDot(device)} />
                        {stateLine(device)}
                    </span>
                </span>
            </div>
            {views.length > 1 && (
                <div class="seg" role="radiogroup" aria-label={device.name}>
                    {views.map((entry) => (
                        <button key={entry} type="button" role="radio" aria-checked={entry === shown} class="seg-btn flex-1" onClick={() => pick(entry)}>
                            {VIEW_TITLE()[entry]}
                        </button>
                    ))}
                </div>
            )}
            {/* Слой разработчика: как пульт узнаёт «этот» компьютер (проверка начисто 18.11, п. 32). */}
            {dev && <p class="w-cap m-0 wrap-anywhere">{probe}</p>}
            <DeviceView device={device} view={view} shown={shown} mine={mine} />
        </>
    );
}

function DeviceView({ device, view, shown, mine }: { device: HomeDevice; view: HomeDevices; shown: View; mine: boolean }) {
    const centre = device.roles.includes('centre');
    if (device.kind === 'station') {
        if (shown === 'room') return <StationRoom device={device} centre={centre} />;
        if (shown === 'device') return <StationDevice />;
        return centre ? (
            <>
                <p class="w-cap m-0">{tr.devices.centreLead}</p>
                <CentreOverview />
            </>
        ) : (
            <EarOverview device={device} view={view} />
        );
    }
    if (device.kind === 'computer') {
        // Своё у компьютера читается у него самого: в окне дома (`?via=`) — фреймом его страницы мимо прохода.
        if (shown === 'computer') return centre ? <ComputerMic /> : VIA ? <OwnFrame page="computer" title={tr.devices.tabThisComputer} /> : <ThisComputer />;
        if (shown === 'homes')
            return (
                <>
                    <p class="w-cap m-0">{tr.devices.otherHomesLead}</p>
                    {VIA ? <OwnFrame page="homes" title={tr.devices.tabOtherHomes} /> : <HelperSide gives={false} />}
                </>
            );
        // Обновления самого этого компьютера: в окне дома (`?via=`) — у его шлюза мимо прохода, а не версия колонки
        // под видом компьютера (owner 20.11).
        if (shown === 'device') return mine && VIA && !centre ? <Updates own /> : mine || centre ? <Updates /> : <TurboDevice device={device} />;
        return centre ? (
            <>
                <p class="w-cap m-0">{tr.devices.centreComputerLead}</p>
                <CentreOverview />
            </>
        ) : (
            <TurboOverview device={device} mine={mine} />
        );
    }
    if (shown === 'device') return <TurboDevice device={device} />;
    return <TurboOverview device={device} mine={mine} />;
}

/** Колонка, что только слышит: куда идёт её голос и есть ли у неё копия дома. */
function EarOverview({ device, view }: { device: HomeDevice; view: HomeDevices }) {
    const centre = centreOf(view);
    const name = centre?.name ?? tr.devices.roleCentre;
    return (
        <Card id="ear" title={tr.devices.voiceGoes}>
            <p class="w-cap m-0">{tr.devices.earLead(name)}</p>
            <div class="w-row">
                <IconEar size={16} />
                <span class="flex-1 min-w-0 w-strong">{tr.devices.voiceGoesTo(name)}</span>
            </div>
            {!device.copy && <p class="w-cap m-0">{tr.devices.copyNone}</p>}
            {can('settings.edit') && device.device && <MakeCentre device={device} />}
        </Card>
    );
}

/** «Сделать центром» (этап 4): у центра — колонка с копией дома становится центром, нынешний центр — её ухом. */
function MakeCentre({ device }: { device: HomeDevice }) {
    const [busy, setBusy] = useState(false);
    const [note, setNote] = useState<{ ok: boolean; text: string } | null>(null);
    const run = async (): Promise<void> => {
        if (!device.device) return;
        if (!device.copy) {
            setNote({ ok: false, text: tr.devices.makeCentreNoCopy });
            return;
        }
        const yes = await ask({ title: tr.devices.makeCentreAsk(device.name), text: tr.devices.makeCentreText, ok: tr.devices.makeCentre });
        if (!yes) return;
        setBusy(true);
        setNote(null);
        try {
            await api.makeCentre(device.device);
            setNote({ ok: true, text: tr.devices.makeCentreDone });
        } catch (e) {
            setNote({ ok: false, text: problemOf(e) });
        } finally {
            setBusy(false);
        }
    };
    return (
        <>
            <p class="w-cap m-0">{tr.devices.makeCentreAbout}</p>
            <div class="w-actions">
                <button class="w-btn" disabled={busy} onClick={() => void run()}>
                    {tr.devices.makeCentre}
                </button>
            </div>
            {note && (
                <p class={`w-cap m-0${note.ok ? '' : ' c-dot-offline'}`} role={note.ok ? 'status' : 'alert'}>
                    {note.text}
                </p>
            )}
        </>
    );
}

/** Турбо: что он даёт дому, по порядку; не пускает регулятор — так и сказано. */
function TurboOverview({ device, mine }: { device: HomeDevice; mine: boolean }) {
    const { view } = useHelpers();
    const panel = usePanel();
    const helper = view?.helpers.find((entry) => `turbo:${entry.id}` === device.id);
    const gives = (helper?.abilities ?? []).filter((ability) => ability.id in ABILITY_ICONS && ability.ready && !helper?.without.includes(ability.id));
    // Голос он дал бы, но голоса на нём нет (проверка начисто 18.11, п. 29): так и сказано, и где поставить — на
    // странице «Этот компьютер», если пульт открыт на нём самом.
    const noVoice = (helper?.abilities ?? []).some((ability) => ability.id === 'voice' && !ability.ready) && !helper?.without.includes('voice');
    const VoiceIcon = ABILITY_ICONS.voice;
    return (
        <>
            <p class="w-cap m-0">{device.kind === 'computer' ? tr.devices.turboLead : tr.devices.turboServerLead}</p>
            {mine && <p class="w-cap m-0">{tr.devices.ownHere}</p>}
            <Card id="gives" title={tr.devices.gives} sub={device.order ? tr.devices.orderOf(device.order) : undefined}>
                {device.kept && <p class="w-cap m-0 c-pill-yandex-fg">{tr.devices.keptAbout}</p>}
                {device.older && <p class="w-cap m-0 c-pill-yandex-fg">{tr.devices.turboOlder}</p>}
                {noVoice && (
                    <div class="w-row">
                        {VoiceIcon && <VoiceIcon size={16} />}
                        <span class="flex-1 min-w-0">
                            <span class="w-strong">{abilityTitle('voice')}</span> <span class="w-cap">— {tr.devices.voiceMissing}</span>
                        </span>
                        {mine ? (
                            // Голоса компьютера — в «Настройки» → «Голос» (проверка начисто 19.11).
                            <button class="w-btn" onClick={() => panel.go('settings', 'voice')}>
                                {tr.devices.voiceInstall}
                            </button>
                        ) : (
                            <span class="w-cap">{tr.devices.voiceInstallThere}</span>
                        )}
                    </div>
                )}
                {gives.length === 0 && !noVoice ? (
                    <p class="w-cap m-0">{tr.devices.givesNone}</p>
                ) : gives.length === 0 ? null : (
                    <div class="w-rows">
                        {gives.map((ability) => {
                            const Icon = ABILITY_ICONS[ability.id];
                            return (
                                <div key={ability.id} class="w-row">
                                    <Icon size={16} />
                                    <span class="flex-1 min-w-0 w-strong">{abilityTitle(ability.id)}</span>
                                </div>
                            );
                        })}
                    </div>
                )}
            </Card>
        </>
    );
}

/** Турбо глазами центра: его версия и «Проверить сейчас». Обновляется Турбо у себя. */
function TurboDevice({ device }: { device: HomeDevice }) {
    const { view } = useHelpers();
    const helper = view?.helpers.find((entry) => `turbo:${entry.id}` === device.id);
    const [note, setNote] = useState<string | null>(null);
    const check = async (): Promise<void> => {
        if (!helper) return;
        setNote(null);
        try {
            await api.checkHelper(helper.id);
        } catch (e) {
            setNote(problemOf(e));
        }
    };
    return (
        <Card id="turbo-device" title={tr.devices.tabDevice}>
            <p class="w-cap m-0">{tr.devices.deviceLead}</p>
            {helper?.version && (
                <div class="w-row">
                    <span class="flex-1 min-w-0 w-strong">DuoWave {helper.version}</span>
                </div>
            )}
            {helper && can('connectors.manage') && (
                <div class="w-actions">
                    <button class="w-btn" onClick={() => void check()}>
                        {tr.helpers.menuCheck}
                    </button>
                </div>
            )}
            {note && (
                <p class="w-cap m-0" role="alert">
                    {note}
                </p>
            )}
        </Card>
    );
}
