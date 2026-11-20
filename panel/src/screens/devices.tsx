// «Настройки → Дом → Устройства» (docs/plans/home-and-devices.md, этап 1; холст 17.11, страница «Устройства»): все
// устройства дома одним списком — центр, колонки, что слышат для него, и Турбо, — и страница каждого, только его вещи.
// Колонка-центр: «Обзор · Комната · Устройство»; колонка, что только слышит: «Обзор · Комната»; компьютер:
// «Обзор · Этот компьютер · Другие дома · Устройство». Домашние разделы (персонаж, «Мозг», люди, подключения)
// показывают дом, здесь — только своё у устройства. Адрес страницы — `#/settings/devices/<id>?view=<вкладка>`.

import type { ComponentChildren, FunctionComponent } from 'preact';
import { useState } from 'preact/hooks';

import { api, can, ownCall, problemOf } from '../api';
import { ask } from '../ui/confirm';
import type { HomeDevice, HomeDevices } from '../api';
import { ABILITY_ICONS, abilityTitle, useHelpers } from '../helpers';
import { centreOf, deviceDot, deviceStateWords, readHomeDevices, useHomeDevices, useOwnHealth, useThisDevice, useThisDeviceProbe } from '../home-devices';
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
            <OtherStations view={view} onJoined={(device) => onOpen(device)} />
            {can('station.view') && !onComputer() && <Stations />}
            {adding && helpers && <AddHelperSheet listed={helpers.helpers} onClose={() => setAdding(false)} onDone={() => setAdding(false)} />}
        </>
    );
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
