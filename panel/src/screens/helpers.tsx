// «Дом» (docs/plans/home-and-helpers.md, блок 1, этап 8; решение владельца 16.11 — «Роли DuoWave»): шлюз и его Турбо
// (людям помощник — «Турбо»). У шлюза — его карточка с «Кто отвечает», Турбо по порядку с переключателями сил, «Если
// Турбо уснёт» («Запасная», регулятор «Что может уходить из дома») и «Где сейчас»; добавить Турбо по тексту подключения
// или по снимку его QR (свой распознаватель, ui/qr-read.ts). У пульта Турбо — «Мои силы» и «Кому помогаю»
// (screens/helper-pc.tsx). Старые адреса «Помощников», «Этого компьютера» и размещения ведут сюда (screens/places.tsx).

import type { ComponentChildren } from 'preact';
import { useRef, useState } from 'preact/hooks';

import { api, can, problemOf, sessionStore } from '../api';
import type { Ability, Away, HelperView, HelpersView, WhereRow } from '../api';
import {
    ABILITIES,
    ABILITY_ICONS,
    AWAYS,
    WHERE_ROWS,
    abilityAbout,
    abilityTitle,
    applyHelpers,
    awayLine,
    awayTitle,
    looksLikeKey,
    parseConnection,
    placeIcon,
    placeTitle,
    readHelpers,
    useHelpers,
} from '../helpers';
import { tr } from '../i18n';
import { IconCheck, IconChevronDown, IconSparkle } from '../icons';
import { usePoll } from '../poll';
import { usePanel } from '../shell/context';
import { useDevMode } from '../shell/dev';
import { ownName } from '../shell/role';
import { since } from '../time';
import { ask } from '../ui/confirm';
import { TextField } from '../ui/fields';
import { decodeQr } from '../ui/qr-read';
import { SaveBar, Sheet, WinRow, WinSection } from '../ui/sheet';
import { Switch } from '../ui/switch';
import { PopMenu } from '../ui/widget';
import { TOP_VIA, VIA } from '../via';
import { onComputer } from '../voice-input';
import { byMode } from '../words';
import { BackupChoice } from './brain';
import { HelperCardWidget } from './desk-widgets';
import { GatewayNameSheet, HelperComputer, HelperSide, NameSheet, TurboPowers } from './helper-pc';
import { fallOf, turboOf } from './turbo-model';

/** A card of «Дом»: its title, a role pill («шлюз», «работает»), a quiet line after the title; `turbo` — the Турбо's violet edge. */
function Card({ id, title, sub, pill, turbo = false, children }: { id: string; title: string; sub?: string; pill?: string; turbo?: boolean; children: ComponentChildren }) {
    return (
        <section class={`card glass px-16 pt-4 pb-14 flex flex-col gap-10${turbo ? ' home-card-turbo' : ''}`} aria-labelledby={`helpers-${id}`}>
            <div class="section-head home-card-head">
                <h2 id={`helpers-${id}`} class="t-title-xs m-0">
                    {title}
                </h2>
                {sub && <span class="w-cap">{sub}</span>}
                {pill && <span class={`pill ${turbo ? 'pill-agent' : 'pill-own'} home-card-pill`}>{pill}</span>}
            </div>
            {children}
        </section>
    );
}

/** A change's outcome, in the card where it was made: the list's, or the regulator's (seen where one taps). */
type Note = { at: 'list' | 'away'; ok: boolean; text: string } | null;

function NoteLine({ note, at }: { note: Note; at: NonNullable<Note>['at'] }) {
    if (note?.at !== at) return null;
    return (
        <p class={`w-cap m-0 wrap-anywhere ${note.ok ? 'c-accent-text' : 'c-dot-offline'}`} role={note.ok ? 'status' : 'alert'}>
            {note.text}
        </p>
    );
}

const isAdmin = (): boolean => sessionStore.get()?.role === 'admin';

/** Abilities this panel knows (a newer gateway may list more). */
const known = (helper: HelperView) => helper.abilities.filter((ability) => ability.id in ABILITY_ICONS);

type Open =
    | { kind: 'add'; repair?: HelperView }
    | { kind: 'without' | 'rename'; helper: HelperView }
    | { kind: 'gateway'; name: string };

/**
 * Настройки → «Дом» (решение владельца 16.11): одна страница вместо «Помощников» и «Этого компьютера». У шлюза — его
 * карточка (кто отвечает на колонке, имя, «Перенести…»), «Отдаю Турбо, когда он на связи» (Турбо по порядку, у каждого —
 * переключатели сил), «Если Турбо уснёт» («Запасная», чем шлюз заменит слух и голос, регулятор «Что может уходить из
 * дома») и «Где сейчас». У пульта Турбо (компьютер, что только помогает) — «Мои силы» и «Кому помогаю».
 */
export function HelpersPage() {
    const panel = usePanel();
    if (panel.role.role === 'helper' && !VIA) return <TurboHome />;
    return <GatewayHome />;
}

/**
 * The centre's page parts (docs/plans/home-and-devices.md, stage 1): «Обзор» — its card, its Турбо in their order with
 * their switches, «Где сейчас»; «Что уходит из дома» — «Если Турбо уснёт» and the regulator, a page of its own in «Дом».
 */
export function CentreOverview() {
    return <GatewayHome part="overview" />;
}

export function AwayPage() {
    return <GatewayHome part="away" />;
}

/** The gateway's «Дом»: on a speaker with its own gateway, on a computer that is the gateway, in the station's window. */
function GatewayHome({ part = 'all' }: { part?: 'all' | 'overview' | 'away' } = {}) {
    const panel = usePanel();
    const dev = useDevMode();
    const { view, older, error } = useHelpers();
    const [note, setNote] = useState<Note>(null);
    const [busy, setBusy] = useState(false);
    const [open, setOpen] = useState<Open | null>(null);
    const placement = panel.health?.placement;
    const station = placement?.role === 'station';
    const manage = can('connectors.manage');
    // The station's window: this computer's own side is one card, whatever the station answers — a 0.13.0 station (no
    // list), one away or still being read — so the computer's own «Дом» is one click away on every computer.
    const computer = TOP_VIA;

    if (older)
        return (
            <>
                {part !== 'away' && <GatewayCard />}
                <OlderGateway />
                {part === 'all' && computer && <ThisComputerCard />}
            </>
        );
    if (!view)
        return (
            <>
                {part !== 'away' && <GatewayCard />}
                <Card id="list" title={tr.helpers.listTitle}>
                    <p class="w-cap m-0" role={error ? 'alert' : undefined}>
                        {error ?? tr.helpers.reading}
                    </p>
                </Card>
                {part === 'all' && computer && <ThisComputerCard />}
            </>
        );

    /**
     * One change: its answer is the whole list; a refusal shows the gateway's words in the card it was made in (`at`)
     * and the list is read again — afresh, so the arrows' swap shown before the answer goes.
     */
    const run = async (job: () => Promise<HelpersView | void>, at: NonNullable<Note>['at'] = 'list'): Promise<void> => {
        setBusy(true);
        setNote(null);
        try {
            const answer = await job();
            if (answer) applyHelpers(answer);
        } catch (e) {
            setNote({ at, ok: false, text: problemOf(e) });
            void readHelpers(true);
        } finally {
            setBusy(false);
        }
    };
    const stored = view.helpers.filter((helper) => !helper.builtin);
    const move = (id: string, by: -1 | 1): void => {
        const from = stored.findIndex((helper) => helper.id === id);
        const to = from + by;
        if (from < 0 || to < 0 || to >= stored.length) return;
        const next = stored.slice();
        [next[from], next[to]] = [next[to], next[from]];
        applyHelpers({ ...view, helpers: [...view.helpers.filter((helper) => helper.builtin), ...next] });
        void run(() => api.orderHelpers(next.map((helper) => helper.id)));
    };
    const act = async (helper: HelperView, what: RowAct): Promise<void> => {
        if (what === 'check') return run(() => api.checkHelper(helper.id));
        if (what === 'remove') {
            const yes = await ask({ title: tr.helpers.removeTitle(helper.name), text: tr.helpers.removeText, ok: tr.helpers.removeOk, danger: true });
            return yes ? run(() => api.removeHelper(helper.id)) : undefined;
        }
        setOpen(what === 'repair' ? { kind: 'add', repair: helper } : { kind: what, helper });
    };
    /** One ability's switch in a row: what is off goes to the next Турбо or a cloud (the gateway's `without`). */
    const give = (helper: HelperView, ability: Ability, on: boolean): Promise<void> =>
        run(() => api.updateHelper(helper.id, { without: on ? helper.without.filter((entry) => entry !== ability) : [...helper.without, ability] }));
    const name = view.gateway?.name ?? panel.health?.gateway?.name;
    const overview = part !== 'away';
    const away = part !== 'overview';
    return (
        <>
            {overview && <GatewayCard name={name} onRename={can('settings.edit') && name ? () => setOpen({ kind: 'gateway', name }) : undefined} />}
            {overview && <Card id="list" title={tr.helpers.listTitle} turbo>
                {view.helpers.length > 0 ? (
                    <div class="w-rows">
                        {view.helpers.map((helper) => {
                            const at = stored.indexOf(helper);
                            return (
                                <HelperRow
                                    key={helper.id}
                                    helper={helper}
                                    order={at >= 0 ? at + 1 : undefined}
                                    away={view.away}
                                    canUp={at > 0}
                                    canDown={at >= 0 && at < stored.length - 1}
                                    manage={manage}
                                    readOnly={false}
                                    busy={busy}
                                    onMove={(by) => move(helper.id, by)}
                                    onAct={(what) => void act(helper, what)}
                                    onGive={(ability, on) => void give(helper, ability, on)}
                                />
                            );
                        })}
                    </div>
                ) : (
                    <div class="flex flex-col gap-2">
                        <span class="w-strong">{tr.helpers.turboNoneTitle}</span>
                        <span class="w-cap">{tr.helpers.turboNoneAbout}</span>
                    </div>
                )}
                {/* In «Микрофон» there is no Yandex to fall back on. */}
                <p class="w-cap m-0">{byMode(tr.helpers.listAbout, tr.helpers.listAboutComputer)}</p>
                {manage && (
                    <div class="w-actions">
                        <button class="w-btn w-btn-ink" disabled={busy} onClick={() => setOpen({ kind: 'add' })}>
                            {tr.helpers.add}
                        </button>
                    </div>
                )}
                <NoteLine note={note} at="list" />
            </Card>}
            {away && <FallCard view={view} station={station}>
                <AwayChoice
                    away={view.away}
                    disabled={!manage || busy}
                    onPick={(away) => void run(() => api.setAway(away), 'away')}
                />
                <NoteLine note={note} at="away" />
                {!manage && <p class="w-cap m-0">{tr.helpers.awayAdminOnly}</p>}
                {!onComputer() && <p class="w-cap m-0">{tr.helpers.awayYandex}</p>}
            </FallCard>}
            {overview && <Card id="where" title={tr.helpers.whereTitle}>
                <WhereList view={view} rows={dev && view.now.some((row) => row.ability === 'catalog') ? [...WHERE_ROWS, 'catalog'] : WHERE_ROWS} station={station} />
            </Card>}
            {part === 'all' && (computer ? <ThisComputerCard /> : placement && !station && <HelperSide />)}
            {open?.kind === 'add' && (
                <AddHelperSheet repair={open.repair} listed={view.helpers} onClose={() => setOpen(null)} onDone={(text) => setNote({ at: 'list', ok: true, text })} />
            )}
            {open?.kind === 'without' && <WithoutSheet helper={open.helper} onClose={() => setOpen(null)} />}
            {open?.kind === 'rename' && (
                <NameSheet
                    title={tr.helpers.nameTitle}
                    name={open.helper.name}
                    onSave={async (value) => applyHelpers(await api.updateHelper(open.helper.id, { name: value }))}
                    onClose={() => setOpen(null)}
                />
            )}
            {open?.kind === 'gateway' && <GatewayNameSheet name={open.name} onClose={() => setOpen(null)} />}
        </>
    );
}

/**
 * The gateway's own card: who it is (the speaker or this computer), who answers on the speaker (the mode switch, which
 * moved here from «Колонка»), the name Турбо see it by, and where the gateway runs — «Перенести…» leads to the move on
 * «Колонка».
 */
function GatewayCard({ name, onRename }: { name?: string; onRename?: () => void }) {
    const panel = usePanel();
    const placement = panel.health?.placement;
    const station = placement?.role === 'station';
    const mic = onComputer();
    const speaker = ownName(panel.health, panel.role) ?? panel.current?.name;
    const title = station || (!mic && placement?.placement !== 'pc') ? (speaker ? tr.core.tabStation(speaker) : tr.core.tabStationNoName) : tr.core.tabThisComputer;
    const lead = station ? tr.helpers.homeLeadStation : mic ? tr.helpers.homeLeadMic : tr.helpers.homeLeadComputer;
    const movable = can('station.view') && !mic && (station || (placement?.stations ?? []).length > 0 || !!placement?.station);
    return (
        <Card id="gateway" title={title} pill={tr.helpers.roleGateway}>
            <p class="w-cap m-0">{lead}</p>
            {name && (
                <div class="w-row helpers-gateway">
                    <span class="flex-1 min-w-0 flex flex-col">
                        <span class="w-strong">{tr.helpers.gatewayName(name)}</span>
                        <span class="w-cap">{tr.helpers.gatewayNameAbout}</span>
                    </span>
                    {onRename && (
                        <button class="w-btn" onClick={onRename}>
                            {tr.helpers.change}
                        </button>
                    )}
                </div>
            )}
            {movable && (
                <div class="w-row">
                    <span class="flex-1 min-w-0 flex flex-col">
                        <span class="w-strong">{station ? tr.helpers.gatewayOnStation : tr.helpers.gatewayOnComputer}</span>
                        <span class="w-cap">{station ? tr.helpers.gatewayOnStationSub : tr.helpers.gatewayOnComputerSub}</span>
                    </span>
                    <button class="w-btn" onClick={() => panel.openItem('settings', 'devices', panel.link.item ?? null, { view: 'device' })}>
                        {tr.helpers.move}
                    </button>
                </div>
            )}
        </Card>
    );
}

/**
 * «Если Турбо уснёт»: «Думает» — «Запасная»; «Слышит» и «Говорит» — чем шлюз заменит Турбо (облако по ключу, Яндекс,
 * Алиса или никто — turbo-model.ts); «Что может уходить из дома» — регулятор (children). Без Турбо — «так сейчас»: кто
 * слышит и говорит по строкам «где сейчас»; у встроенного Турбо — «если он не справится».
 */
function FallCard({ view, station, children }: { view: HelpersView; station: boolean; children: ComponentChildren }) {
    const panel = usePanel();
    const [cloud, setCloud] = useState<{ ear: boolean; voice: boolean }>({ ear: false, voice: false });
    usePoll({
        on: can('connectors.view'),
        every: 30_000,
        version: panel.version,
        read: (signal) =>
            api.connectors().then(({ connectors }) => {
                if (signal.aborted) return;
                const on = (id: string) =>
                    connectors.some((entry) => entry.manifest.id === id && entry.manifest.cloud && (entry.status.state === 'ready' || entry.status.state === 'configured'));
                setCloud({ ear: on('asr'), voice: on('tts') });
            }),
    });
    const turbo = turboOf(view);
    const t = tr.helpers;
    const input = { station, computer: onComputer(), mode: panel.health?.panelMode, away: view.away, cloudEar: cloud.ear, cloudVoice: cloud.voice };
    const fallWords = (ability: 'hearing' | 'voice'): { text: string; cls: string } => {
        const { by } = fallOf(ability, input);
        if (by === 'cloud') return { text: ability === 'hearing' ? t.fallCloudEar : t.fallCloudVoice, cls: 'pill-yandex' };
        if (by === 'yandex') return { text: ability === 'hearing' ? t.fallYandexEar : t.fallAliceVoice, cls: 'pill-yandex' };
        if (by === 'text') return { text: t.fallText, cls: 'pill-off' };
        return { text: t.fallNone, cls: 'pill-off' };
    };
    const [title, sub] = !turbo ? [t.fallTitleNone, t.fallSubNone] : turbo.builtin ? [t.fallTitleBuiltin, t.fallSubBuiltin] : [t.fallTitle, t.fallSub];
    const modeFalls = !!turbo && !turbo.builtin && fallOf('hearing', input).modeFalls;
    return (
        <Card id="fall" title={title} sub={sub}>
            <div class="w-rows">
                <div class="w-row fall-row">
                    <IconSparkle size={16} />
                    <span class="flex-1 min-w-0 flex flex-col gap-6">
                        <span class="flex flex-col">
                            <span class="w-strong">{abilityTitle('model', true)}</span>
                            <span class="w-cap">{turbo ? t.fallThinksSub : t.fallThinksSubNone}</span>
                        </span>
                        {can('connectors.view') && <BackupChoice />}
                    </span>
                </div>
                {(['hearing', 'voice'] as const).map((ability) => {
                    const Icon = ABILITY_ICONS[ability];
                    // With a Турбо: what stands in when it sleeps; without: who does it now, as the gateway says.
                    const giver = turbo && turbo.gives.includes(ability) ? fallWords(ability) : giverOf(view.now.find((row) => row.ability === ability), ability, view, station);
                    return (
                        <div key={ability} class="w-row fall-row">
                            <Icon size={16} />
                            <span class="flex-1 min-w-0 w-strong">{abilityTitle(ability, true)}</span>
                            <span class={`pill ${giver.cls} where-giver`}>
                                <span>{giver.text}</span>
                            </span>
                        </div>
                    );
                })}
            </div>
            {modeFalls && <p class="w-cap m-0">{t.fallModeBecomes}</p>}
            <div class="flex flex-col gap-6">
                <span class="w-strong">{t.awayTitle}</span>
                {children}
            </div>
        </Card>
    );
}

/** The Турбо's own «Дом» — a computer that only helps: «Мои силы» (what it gives, ready or not) and «Кому помогаю». */
export function TurboHome() {
    const panel = usePanel();
    return (
        <>
            <p class="w-cap m-0">{tr.helpers.homeLeadTurbo}</p>
            <TurboPowers />
            <HelperComputer home />
            {can('station.view') && (
                <Card id="become" title={tr.helpers.becomeGateway}>
                    <div class="w-row">
                        <span class="flex-1 min-w-0 w-cap">{tr.helpers.becomeGatewaySub}</span>
                        <button class="w-btn" onClick={() => panel.go('settings', 'devices')}>
                            {tr.helpers.move}
                        </button>
                    </div>
                </Card>
            )}
        </>
    );
}

/** A 0.11 gateway has no list of helpers: its computer as the old card, and why. */
function OlderGateway() {
    return (
        <Card id="older" title={tr.helpers.listTitle}>
            <HelperCardWidget />
            <p class="w-cap m-0">{tr.helpers.olderGateway}</p>
        </Card>
    );
}

/**
 * «Этот компьютер — Турбо» at the bottom of the station's «Дом» in the computer's window: its own side — powers, who it
 * helps, the connection key — is the computer's own «Дом», one click away (`?local`).
 */
function ThisComputerCard() {
    return (
        <Card id="this-computer" title={tr.helpers.thisComputerTitle}>
            <p class="w-cap m-0">{tr.helpers.thisComputerLink}</p>
            <div class="w-actions">
                <a class="w-btn" href={`${location.pathname}?local#/settings/helpers`}>
                    {tr.helpers.thisComputerOpen}
                </a>
            </div>
        </Card>
    );
}

type RowAct = 'check' | 'without' | 'rename' | 'repair' | 'remove';

function stateWords(helper: HelperView): string {
    const t = tr.helpers;
    if (helper.builtin) return t.stateHere;
    if (!helper.enabled || helper.state === 'off') return t.stateOff;
    if (helper.state === 'self') return t.stateSelf;
    if (helper.state === 'twin') return t.stateTwin;
    if (helper.state === 'up') return helper.since ? t.stateUp(since(helper.since, true)) : t.stateUpNow;
    if (helper.state === 'down') return helper.since ? t.stateDown(since(helper.since, true)) : t.stateDownNow;
    return t.stateUnknown;
}

/** The helper's own panel, for a helper at home on the line reached by its address (a link's port serves no panel). */
function panelOf(helper: HelperView): string | null {
    if (helper.kind !== 'home' || helper.state !== 'up' || !helper.url || helper.link || TOP_VIA) return null;
    try {
        return `${new URL(helper.url).origin}/panel`;
    } catch {
        return null;
    }
}

function HelperRow({
    helper,
    order,
    away,
    canUp,
    canDown,
    manage,
    readOnly,
    busy,
    onMove,
    onAct,
    onGive,
}: {
    helper: HelperView;
    /** Its place in the order (1 — asked first); the built-in has none. */
    order?: number;
    away: Away;
    canUp: boolean;
    canDown: boolean;
    manage: boolean;
    readOnly: boolean;
    busy: boolean;
    onMove: (by: -1 | 1) => void;
    onAct: (what: RowAct) => void;
    /** One ability's switch: give it to the gateway or not. */
    onGive: (ability: Ability, on: boolean) => void;
}) {
    const t = tr.helpers;
    const Icon = placeIcon(helper);
    const moving = manage && !readOnly && !busy;
    const pin = helper.link?.pin ?? '';
    const linkDown = helper.link?.state === 'down' ? helper.link.reason : undefined;
    // Warnings (kept out, the last check's words, why the link is down) in the warning colour; facts plain. A linked
    // helper's error is its link's reason (stage 7): said once. Not checked yet with words — no key stored for it
    // («Подключить заново» gives one): said unless the regulator keeps it out, which its own line says.
    const failed = helper.state === 'down' || helper.state === 'twin' || (helper.state === 'unknown' && helper.allowed !== false);
    const captions = [
        { id: 'blocked', text: helper.allowed === false && t.blocked(awayTitle(away)), warn: true },
        { id: 'older', text: helper.contract === 0 && helper.version && t.older(helper.version), warn: false },
        { id: 'link', text: helper.link && [t.viaLink, pin.startsWith('public:') ? t.linkPublic(pin.slice(7)) : t.linkPinned].join(' · '), warn: false },
        { id: 'link-down', text: linkDown, warn: true },
        { id: 'error', text: failed && helper.error !== linkDown && helper.error, warn: true },
    ].filter((line): line is { id: string; text: string; warn: boolean } => !!line.text);
    // Read-only speaker: only its own «Компьютер дома» may be set up here; «Проверить сейчас» works for every row.
    const own = !readOnly || helper.id === 'home-computer';
    const link = panelOf(helper);
    const items = [
        { label: t.menuCheck, act: () => onAct('check') },
        ...(manage && own ? [{ label: t.menuWithout, act: () => onAct('without') }] : []),
        ...(manage && !readOnly ? [{ label: t.menuRename, act: () => onAct('rename') }] : []),
        // A plain row too: its key may stop working (deleted on the helper), and a new one goes in here.
        ...(manage && own ? [{ label: t.menuRepair, act: () => onAct('repair') }] : []),
        ...(link ? [{ label: t.menuPanel, href: link }] : []),
        ...(manage && !readOnly ? [{ label: t.menuRemove, act: () => onAct('remove') }] : []),
    ];
    // Its powers as switches right in the row (the board «Дом»): model, hearing, voice, heavy apps; the rest — «Не брать
    // у него…». Only where it may be set up here; elsewhere — the icons, as they are.
    const switches = manage && own && !helper.builtin;
    return (
        <div class="helpers-row">
            <span class={`helpers-icon${order ? ' helpers-order' : ''}`} aria-hidden="true">
                {order ?? <Icon size={16} />}
            </span>
            <span class="helpers-main">
                <span class="helpers-name">
                    <b>{helper.name}</b>
                    {helper.builtin && helper.name.toLowerCase() !== t.thisComputer.toLowerCase() && <span class="pill pill-own">{t.thisComputer}</span>}
                </span>
                <span class="w-cap">
                    {placeTitle(helper.kind)} · {stateWords(helper)}
                </span>
                {captions.map((line) => (
                    <span key={line.id} class={`w-cap${line.warn ? ' c-pill-yandex-fg' : ''}`}>
                        {line.text}
                    </span>
                ))}
                {switches ? <AbilitySwitches helper={helper} disabled={busy} onGive={onGive} /> : <AbilityIcons helper={helper} />}
            </span>
            {!helper.builtin && (
                <span class="helpers-tools">
                    <button class="helpers-move helpers-up" aria-label={t.moveUp(helper.name)} disabled={!moving || !canUp} onClick={() => onMove(-1)}>
                        <IconChevronDown size={16} />
                    </button>
                    <button class="helpers-move" aria-label={t.moveDown(helper.name)} disabled={!moving || !canDown} onClick={() => onMove(1)}>
                        <IconChevronDown size={16} />
                    </button>
                    <PopMenu label={t.menuLabel(helper.name)} items={items} />
                </span>
            )}
        </div>
    );
}

/** The powers a Турбо gives a question and heavy apps — the switches in its row, in this order. */
const ROW_SWITCHES: Ability[] = ['model', 'hearing', 'voice', 'apps'];

/** Its declared powers as switches: on — the gateway takes it; off — the next Турбо or a cloud gives it. */
function AbilitySwitches({ helper, disabled, onGive }: { helper: HelperView; disabled: boolean; onGive: (ability: Ability, on: boolean) => void }) {
    const list = ROW_SWITCHES.flatMap((id) => helper.abilities.filter((ability) => ability.id === id));
    if (list.length === 0) return <AbilityIcons helper={helper} />;
    return (
        <span class="helpers-switches" role="group" aria-label={tr.helpers.abilitiesSwitchLabel(helper.name)}>
            {list.map((ability) => {
                const Icon = ABILITY_ICONS[ability.id];
                const title = abilityTitle(ability.id);
                const details = ability.details;
                const about = ability.ready ? [details?.model, details?.voice].filter(Boolean).join(' · ') || abilityAbout(ability.id) : tr.helpers.notReady;
                return (
                    <span key={ability.id} class="w-row helpers-switch">
                        <Icon size={16} />
                        <span class="flex-1 min-w-0 flex flex-col">
                            <span class="w-text">{title}</span>
                            <span class="w-cap wrap-anywhere">{about}</span>
                        </span>
                        <Switch
                            on={helper.enabled && !helper.without.includes(ability.id)}
                            label={`${title}: ${helper.name}`}
                            disabled={disabled || !helper.enabled}
                            onChange={(on) => onGive(ability.id, on)}
                        />
                    </span>
                );
            })}
        </span>
    );
}

/** The declared abilities as icons: taken, not taken (switched off), not ready. */
function AbilityIcons({ helper }: { helper: HelperView }) {
    const list = known(helper);
    if (list.length === 0) return null;
    return (
        <span class="helpers-abilities" role="group" aria-label={tr.helpers.abilitiesLabel}>
            {list.map((ability) => {
                const Icon = ABILITY_ICONS[ability.id];
                const title = abilityTitle(ability.id);
                const skipped = !helper.enabled || helper.without.includes(ability.id);
                const label = skipped ? tr.helpers.abilitySkipped(title) : ability.ready ? tr.helpers.abilityTaken(title) : tr.helpers.abilityNotReady(title);
                return (
                    <span key={ability.id} role="img" aria-label={label} title={label} class={skipped || !ability.ready ? 'helpers-ability-off' : undefined}>
                        <Icon size={16} />
                    </span>
                );
            })}
        </span>
    );
}

/** «Что может уходить из дома»: three positions, each with one line of what changes. */
function AwayChoice({ away, disabled, onPick }: { away: Away; disabled: boolean; onPick: (away: Away) => void }) {
    return (
        <div class="away-options" role="radiogroup" aria-label={tr.helpers.awayTitle}>
            {AWAYS.map((value) => (
                <button key={value} type="button" role="radio" class="away-option" aria-checked={away === value} disabled={disabled} onClick={() => away !== value && onPick(value)}>
                    <span class="away-mark" aria-hidden="true">
                        {away === value && <IconCheck size={16} />}
                    </span>
                    <span class="flex flex-col gap-2 min-w-0">
                        <span class="w-strong">{awayTitle(value)}</span>
                        <span class="w-cap">{awayLine(value)}</span>
                    </span>
                </button>
            ))}
        </div>
    );
}

function giverOf(row: WhereRow | undefined, ability: Ability, view: HelpersView, station: boolean): { text: string; cls: string } {
    const t = tr.helpers;
    if (!row) return { text: t.whereUnknown, cls: 'pill-off' };
    if (row.by === 'helper') {
        const helper = view.helpers.find((entry) => entry.id === row.helper);
        const home = helper ? helper.builtin || helper.kind === 'home' : row.reach === 'home';
        return { text: row.name ?? helper?.name ?? t.whereUnknown, cls: home ? 'pill-own' : 'pill-agent' };
    }
    // «Мозг» answering with a model of no helper and no cloud (a person's own server at home) is `here` with its title:
    // the title, not the place — on a speaker «сама колонка» would claim a model it does not run.
    if (row.by === 'here') return { text: row.name ?? (station ? t.whereHereStation : t.whereHereComputer), cls: 'pill-own' };
    if (row.by === 'cloud') return { text: t.whereCloud(row.name ?? ''), cls: 'pill-yandex' };
    if (row.by === 'yandex') return { text: ability === 'voice' ? t.whereAliceVoice : t.whereYandex, cls: 'pill-yandex' };
    // Nobody hears, thinks or speaks — a fault; nobody passes messages — an ordinary home without a Telegram bot.
    return { text: t.whereNone, cls: ability === 'hearing' || ability === 'model' || ability === 'voice' ? 'pill-error' : 'pill-off' };
}

/**
 * «Где сейчас»: who gives each ability now and, when it is not the first way, the gateway's reason. `short` (the
 * narrow desk widget): the giver under the ability's name, so neither is cut.
 */
function WhereList({ view, rows, station, short = false }: { view: HelpersView; rows: Ability[]; station: boolean; short?: boolean }) {
    return (
        <div class={`w-rows${short ? ' where-short' : ''}`}>
            {rows.map((ability) => {
                const row = view.now.find((entry) => entry.ability === ability);
                const Icon = ABILITY_ICONS[ability];
                const giver = giverOf(row, ability, view, station);
                return (
                    <div key={ability} class="w-row where-row" data-ability={ability}>
                        <Icon size={16} />
                        <span class="flex-1 min-w-0 flex flex-col gap-2">
                            <span class="where-line">
                                <span class="w-text">{abilityTitle(ability, true)}</span>
                                {/* The pill stays a flex box: its colour dot (the theme's ::before) says who gives; the
                                    words in their own span, so they can be cut or wrapped. */}
                                <span class={`pill ${giver.cls} where-giver`}>
                                    <span>{giver.text}</span>
                                </span>
                            </span>
                            {row && !row.first && row.why && <span class="w-cap">{row.why}</span>}
                        </span>
                    </div>
                );
            })}
        </div>
    );
}

/** «Что брать у «…»»: the master switch, then each ability; saved at once, a refusal (the model still answers) shown here. */
function WithoutSheet({ helper, onClose }: { helper: HelperView; onClose: () => void }) {
    const t = tr.helpers;
    const [enabled, setEnabled] = useState(helper.enabled);
    const [without, setWithout] = useState<Ability[]>(helper.without);
    const [busy, setBusy] = useState(false);
    const [error, setError] = useState<string | null>(null);
    const declared = new Map(helper.abilities.map((ability) => [ability.id, ability]));
    const save = async (): Promise<void> => {
        setBusy(true);
        setError(null);
        try {
            applyHelpers(await api.updateHelper(helper.id, { enabled, without }));
            onClose();
        } catch (e) {
            setError(problemOf(e));
            setBusy(false);
        }
    };
    // The switches wait for «Сохранить» (one change, as before): nothing changed — no bar, the window closes by ✕.
    const dirty = enabled !== helper.enabled || !sameSet(without, helper.without);
    const revert = (): void => {
        setEnabled(helper.enabled);
        setWithout(helper.without);
        setError(null);
    };
    const lastError = helper.state === 'down' && helper.error;
    return (
        <Sheet
            title={t.withoutTitle(helper.name)}
            meta={
                <>
                    <span class={`pill ${stateTone(helper)}`}>{stateWords(helper)}</span>
                    <span class="win-chip">{placeTitle(helper.kind)}</span>
                    {helper.version && <span class="win-chip">DuoWave {helper.version}</span>}
                </>
            }
            onClose={onClose}
            footer={dirty || busy ? <SaveBar dirty={dirty} busy={busy} saveLabel={t.save} onRevert={revert} onSave={() => void save()} /> : undefined}
        >
            <p class="win-lead">{t.withoutAbout}</p>
            {lastError && <p class="win-callout m-0 wrap-anywhere">{t.lastError(lastError)}</p>}
            <div class="win-group">
                <label class="win-row">
                    <span class="win-row-main">
                        <span class="win-row-t">{t.takeFrom}</span>
                        {!enabled && <span class="win-row-s">{t.takeFromOff}</span>}
                    </span>
                    <Switch on={enabled} label={t.takeFrom} onChange={setEnabled} />
                </label>
            </div>
            <WinSection title={t.abilitiesLabel} group>
                {ABILITIES.map((id) => {
                    const ability = declared.get(id);
                    const Icon = ABILITY_ICONS[id];
                    const details = ability?.details;
                    const about = ability
                        ? [abilityAbout(id), details?.model, details?.voice, details?.languages?.join(', '), !ability.ready && t.notReady].filter(Boolean).join(' · ')
                        : t.notDeclared;
                    return (
                        <WinRow key={id} icon={<Icon size={16} />} title={abilityTitle(id)} sub={about}>
                            <Switch
                                on={!!ability && !without.includes(id)}
                                label={abilityTitle(id)}
                                disabled={!ability || !enabled}
                                onChange={(on) => setWithout(on ? without.filter((entry) => entry !== id) : [...without, id])}
                            />
                        </WinRow>
                    );
                })}
            </WinSection>
            {error && (
                <p class="win-callout m-0 wrap-anywhere" role="alert">
                    {error}
                </p>
            )}
        </Sheet>
    );
}

/** The same abilities, in any order. */
const sameSet = (a: Ability[], b: Ability[]): boolean => a.length === b.length && a.every((id) => b.includes(id));

/** The state pill's tone in a window's header: on the line — own, asleep — error, the rest — off. */
function stateTone(helper: HelperView): string {
    if (!helper.enabled || helper.state === 'off') return 'pill-off';
    if (helper.state === 'up') return 'pill-own';
    if (helper.state === 'down') return 'pill-error';
    return 'pill-off';
}

type Detector = new (options: { formats: string[] }) => { detect: (source: CanvasImageSource) => Promise<Array<{ rawValue: string }>> };

/**
 * A photo → the text of its QR code: the browser's own reader when it has one (fast), else — or when it finds nothing —
 * ours, on a copy at most 1024 px on its long side; a larger photo that gave nothing is tried once more at 2048 px (a
 * QR far off in the frame).
 */
async function readQrPhoto(file: File): Promise<string | null> {
    const url = URL.createObjectURL(file);
    try {
        let source: CanvasImageSource;
        let [width, height] = [0, 0];
        try {
            const bitmap = await createImageBitmap(file);
            [source, width, height] = [bitmap, bitmap.width, bitmap.height];
        } catch {
            const image = new Image();
            image.src = url;
            await image.decode();
            [source, width, height] = [image, image.naturalWidth, image.naturalHeight];
        }
        for (const limit of [1024, 2048]) {
            const scale = Math.min(1, limit / Math.max(width, height));
            const canvas = document.createElement('canvas');
            canvas.width = Math.max(1, Math.round(width * scale));
            canvas.height = Math.max(1, Math.round(height * scale));
            const context = canvas.getContext('2d', { willReadFrequently: true });
            if (!context) return null;
            context.drawImage(source, 0, 0, canvas.width, canvas.height);
            const Native = (window as unknown as { BarcodeDetector?: Detector }).BarcodeDetector;
            if (Native && limit === 1024) {
                try {
                    const found = (await new Native({ formats: ['qr_code'] }).detect(canvas))[0]?.rawValue;
                    if (found) return found;
                } catch {
                    // no QR reader in this browser after all: ours below
                }
            }
            const text = decodeQr(context.getImageData(0, 0, canvas.width, canvas.height));
            if (text !== null || scale === 1 || Math.max(width, height) < 1.5 * limit) return text;
        }
        return null;
    } finally {
        URL.revokeObjectURL(url);
    }
}

/**
 * «Добавить помощника», two steps: the address and the connection key (typed, pasted whole, or read from a photo of
 * its QR) → «Проверить» asks the gateway what the helper gives, storing nothing → what to take from it, the name →
 * «Добавить». «Подключить заново» (`repair`) starts from the row's address and renews only its link: the row keeps its
 * name, place and «Не брать у него…» (shown, not changed here — the gateway replaces the link alone), and the check
 * before saving names it too (`replace`), so a helper whose certificate or address changed is not refused as one
 * already listed. A helper the list has already, with the very certificate pinned (the check's `matched`, stage 7), is
 * that row connected again the same way: the gateway would renew it in place and keep its name and choices whatever
 * is typed here, so the sheet says which row it is and saves it as «Подключить заново». «Компьютер дома» is the pair's
 * own plain entry, never paired by a connection key (D27): its new address or API key from the computer goes through
 * the pair's door, which keeps the row as it is and looks at the computer at once; no key typed — the stored one
 * stays. `listed` — the list as shown, for the row `matched` names; `onDone` — the words for the page.
 */
export function AddHelperSheet({
    repair,
    listed,
    onClose,
    onDone,
}: {
    repair?: HelperView;
    listed: HelperView[];
    onClose: () => void;
    onDone: (text: string) => void;
}) {
    const t = tr.helpers;
    const door = repair?.id === 'home-computer' && !repair.link;
    const [url, setUrl] = useState(repair?.url ?? '');
    const [key, setKey] = useState('');
    const [name, setName] = useState('');
    const [found, setFound] = useState<HelperView | null>(null);
    /** The listed row the check found this helper to be (never with `repair`): saved as that row's «Подключить заново». */
    const [matched, setMatched] = useState<string | null>(null);
    const [without, setWithout] = useState<Ability[]>(repair?.without ?? []);
    const [busy, setBusy] = useState<'check' | 'add' | 'scan' | null>(null);
    const [error, setError] = useState<string | null>(null);
    const photo = useRef<HTMLInputElement>(null);
    const address = url.trim().replace(/\/+$/, '');
    /** Only the link is renewed: of the row repaired, or of the one matched. */
    const renewing = !!repair || matched !== null;
    /** That row as listed; a matched one the list does not have yet comes with the read afresh (until then: the helper's own name). */
    const row = repair ?? listed.find((helper) => helper.id === matched);
    /** The choices shown: a renewed row's, as they stay; a new helper's, to make. */
    const kept = row ?? { enabled: true, without };

    /**
     * Text put into a field. Pasted (more than one character at once): the whole connection text fills both fields, a
     * key alone in the address field or an address alone in the key field goes where it belongs. Typed letter by letter,
     * it stays where it is typed: a split halfway would send the next letters into the other field's text.
     */
    const typed = (text: string, field: 'url' | 'key'): void => {
        const pasted = text.length - (field === 'url' ? url : key).length > 1;
        const parsed = pasted ? parseConnection(text) : {};
        if (parsed.url && parsed.key) {
            setUrl(parsed.url);
            setKey(parsed.key);
        } else if (pasted && field === 'url' && !parsed.url && looksLikeKey(text)) {
            setUrl('');
            setKey(text.trim());
        } else if (field === 'key' && parsed.url) {
            setUrl(parsed.url);
            setKey('');
        } else (field === 'url' ? setUrl : setKey)(text);
    };
    const check = async (): Promise<void> => {
        if (!/^https?:\/\/\S+$/i.test(address)) return setError(t.addBadAddress);
        setBusy('check');
        setError(null);
        try {
            const answer = await api.probeHelper(address, key.trim(), repair?.id);
            // Listed already: saved, it is that row connected again, under its own name — none is put in here.
            const listedAs = repair ? null : (answer.matched ?? null);
            if (listedAs && !listed.some((helper) => helper.id === listedAs)) void readHelpers(true);
            setMatched(listedAs);
            setFound(answer.helper);
            if (!repair && !listedAs) setName((given) => (given.trim() ? given : answer.helper.name));
            setWithout(repair?.without ?? []);
        } catch (e) {
            setError(problemOf(e));
        } finally {
            setBusy(null);
        }
    };
    /** «Назад»: the helper's own name that «Проверить» put in goes too — another helper checked next gets its own. */
    const back = (): void => {
        if (name === found?.name) setName('');
        setFound(null);
        setMatched(null);
    };
    const add = async (): Promise<void> => {
        if (!found) return;
        setBusy('add');
        setError(null);
        try {
            const given = name.trim();
            const link = { url: address, key: key.trim() };
            const replace = repair?.id ?? matched;
            const answer = await api.addHelper(replace ? { ...link, replace } : { ...link, name: given || undefined, without });
            applyHelpers(answer);
            // The gateway's word on what it did (a row renewed in place keeps its name), with the name it stored.
            const saved = answer.helpers.find((helper) => helper.id === (answer.matched ?? answer.id));
            const shown = saved?.name ?? row?.name ?? (given || found.name);
            onDone(answer.matched ? t.repaired(shown) : t.addAdded(shown));
            onClose();
        } catch (e) {
            setError(problemOf(e));
            setBusy(null);
        }
    };
    /** «Компьютер дома» through the door: stored either way; a computer that does not take the key says why here. */
    const relink = async (): Promise<void> => {
        if (!repair) return;
        if (!/^https?:\/\/\S+$/i.test(address)) return setError(t.addBadAddress);
        const given = key.trim();
        if (given && !given.startsWith('vgw_')) return setError(t.repairDoorKey);
        setBusy('add');
        setError(null);
        try {
            const answer = await api.linkHomeServer({ url: address, key: given || undefined });
            await readHelpers(true);
            if (answer.state === 'down') {
                setError(answer.error ?? t.stateDownNow);
                setBusy(null);
                return;
            }
            onDone(t.repaired(repair.name));
            onClose();
        } catch (e) {
            setError(problemOf(e));
            setBusy(null);
        }
    };
    const scan = async (file: File): Promise<void> => {
        setBusy('scan');
        setError(null);
        try {
            const parsed = parseConnection((await readQrPhoto(file)) ?? '');
            if (!parsed.url && !parsed.key) setError(t.addScanNone);
            if (parsed.url) setUrl(parsed.url);
            if (parsed.key) setKey(parsed.key);
        } catch {
            setError(t.addScanNone);
        } finally {
            setBusy(null);
        }
    };
    const abilities = found ? known(found) : [];
    const nothing = !abilities.some((ability) => ability.ready);
    const spin = (what: NonNullable<typeof busy>) => busy === what && <span class="spinner spinner-sm" aria-hidden="true" />;
    return (
        <Sheet
            title={found ? t.addGives(found.name) : repair ? t.menuRepair : t.addTitle}
            meta={
                found ? (
                    <>
                        <span class="win-chip">{placeTitle(found.kind)}</span>
                        {found.version && <span class="win-chip">DuoWave {found.version}</span>}
                    </>
                ) : repair ? (
                    <span class="win-chip">{repair.name}</span>
                ) : undefined
            }
            onClose={onClose}
            footer={
                door ? (
                    <button class="btn btn-main" disabled={busy !== null || !address} aria-busy={busy === 'add' || undefined} onClick={() => void relink()}>
                        {spin('add')}
                        {t.menuRepair}
                    </button>
                ) : found ? (
                    <>
                        <button class="btn btn-plain" disabled={busy !== null} onClick={back}>
                            {t.addBack}
                        </button>
                        <button class="btn btn-main" disabled={busy !== null} aria-busy={busy === 'add' || undefined} onClick={() => void add()}>
                            {spin('add')}
                            {renewing ? t.menuRepair : t.addSave}
                        </button>
                    </>
                ) : (
                    <button class="btn btn-main" disabled={busy !== null || !address || !key.trim()} aria-busy={busy === 'check' || undefined} onClick={() => void check()}>
                        {spin('check')}
                        {busy === 'check' ? t.addChecking : t.addCheck}
                    </button>
                )
            }
        >
            {found ? (
                <>
                    {/* Before the switches: «Добавить» became «Подключить заново» of a row already there. */}
                    {matched !== null && <p class="win-callout m-0">{t.addListed(row?.name ?? found.name)}</p>}
                    {abilities.length > 0 && (
                        <div class="win-group">
                            {abilities.map((ability) => {
                                const Icon = ABILITY_ICONS[ability.id];
                                return (
                                    <WinRow key={ability.id} icon={<Icon size={16} />} title={abilityTitle(ability.id)} sub={ability.ready ? abilityAbout(ability.id) : t.notReady}>
                                        <Switch
                                            on={kept.enabled && !kept.without.includes(ability.id)}
                                            label={abilityTitle(ability.id)}
                                            disabled={renewing}
                                            onChange={(on) => setWithout(on ? without.filter((entry) => entry !== ability.id) : [...without, ability.id])}
                                        />
                                    </WinRow>
                                );
                            })}
                        </div>
                    )}
                    {nothing && <p class="win-callout is-calm m-0">{t.addGivesNothing}</p>}
                    {/* A computer gives its own home the apps always (a 0.11 one lists them too): none — another home,
                        given hearing, voice and the model until the computer's owner gives more (owner's decision 15). */}
                    {!found.abilities.some((ability) => ability.id === 'apps') && <p class="win-note">{t.addOtherHome}</p>}
                    {found.contract === 0 && <p class="win-callout is-calm m-0">{t.addOlder(found.version)}</p>}
                    {renewing ? <p class="win-text m-0">{t.repairKeeps(row?.name ?? found.name)}</p> : <TextField label={t.nameField} value={name} onInput={setName} />}
                </>
            ) : (
                <form
                    class="flex flex-col gap-14"
                    onSubmit={(e) => {
                        e.preventDefault();
                        if (door) void relink();
                        else if (address && key.trim()) void check();
                    }}
                >
                    <p class="win-lead">{door ? t.repairDoorAbout : t.addAbout}</p>
                    <TextField
                        label={t.addAddress}
                        value={url}
                        placeholder={door ? 'http://192.0.2.10:7780' : 'https://192.0.2.10:7781'}
                        type="url"
                        inputMode="url"
                        onInput={(text) => typed(text, 'url')}
                    />
                    <label class="flex flex-col gap-6">
                        <span class="win-label">{door ? tr.placement.pcKey : t.addKey}</span>
                        <input
                            class="field key-text"
                            value={key}
                            placeholder={door ? repair?.key?.hint : undefined}
                            autocomplete="off"
                            spellcheck={false}
                            autocapitalize="off"
                            autocorrect="off"
                            onInput={(e) => typed((e.target as HTMLInputElement).value, 'key')}
                        />
                        {/* The QR and the pasted text carry a connection key, which the pair's door does not take. */}
                        {!door && <span class="win-hint">{t.addPaste}</span>}
                    </label>
                    {!door && (
                        <>
                            <button
                                type="button"
                                class="btn btn-plain btn-sm self-start"
                                disabled={busy !== null}
                                aria-busy={busy === 'scan' || undefined}
                                onClick={() => photo.current?.click()}
                            >
                                {spin('scan')}
                                {busy === 'scan' ? t.addScanBusy : t.addScan}
                            </button>
                            <input
                                ref={photo}
                                type="file"
                                accept="image/*"
                                capture="environment"
                                hidden
                                onChange={(e) => {
                                    const input = e.target as HTMLInputElement;
                                    const file = input.files?.[0];
                                    input.value = '';
                                    if (file) void scan(file);
                                }}
                            />
                        </>
                    )}
                    {!repair && <TextField label={t.addName} value={name} onInput={setName} />}
                </form>
            )}
            {error && (
                <p class="win-callout m-0 wrap-anywhere" role="alert">
                    {error}
                </p>
            )}
        </Sheet>
    );
}
