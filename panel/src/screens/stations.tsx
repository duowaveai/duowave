import { useId, useRef, useState } from 'preact/hooks';

import { LookSource, LookVariant, SessionSnapshot, Station, StationLook, StationPresence, api, can, problemOf } from '../api';
import { here } from '../availability';
import { IconCheck, IconChevronDown, IconPlus } from '../icons';
import { Reach, dotClass, isReachable, reachOf } from '../presence';
import { StationDrawing, colorInfo, drawingOf } from '../stations';
import { timeAgo } from '../time';
import { tr } from '../i18n';
import { Sheet, useDialogFocus } from '../ui/sheet';
import { ConnectSheet, RestoreSheet } from './connect';
import { Switch } from '../ui/switch';

// Колонки дома: переключатель в шапке, окно «Колонки» со списком и видом выбранной, мастер первой настройки и
// плашка о новой колонке на дашборде. Макет — блок pickerOpen в docs/design/glass/source/GlassDash.dc.html.
// Что колонка сообщает о себе (модель, цвет), решает шлюз (gateway/src/stations/looks.rs); человек выбирает остальное.

/** Имя, с которым колонка приходит в первый раз (StationsService.admit). */
const DEFAULT_NAME = 'Колонка';

/** Имя для людей: своё, а пока его нет — модель («Лайт 2»). */
export function displayName(station: Station | null | undefined, drawing: StationDrawing): string {
    return !station || station.name === DEFAULT_NAME ? drawing.title : station.name;
}

/** Выбор вида до сохранения: null — оставить как решит шлюз. */
export interface LookValue {
    model: string | null;
    color: string | null;
    /** Наклейка-котик (только Лайт 2); null — как сохранено. */
    sticker?: boolean | null;
}

/** Вид с выбором поверх — так, как его решит шлюз (looks.rs, look_of): для рисунка до сохранения. */
export function previewLook(look: StationLook, value: LookValue): { variant: LookVariant; color: string; colors: string[] } {
    const variant =
        look.variants.find((entry) => entry.id === value.model) ??
        look.variants.find((entry) => entry.id === look.model) ??
        look.variants[0];
    const colors = look.colorFrom === 'station' ? look.colors : variant.colors;
    const color = look.colorFrom === 'station' ? look.color : value.color && colors.includes(value.color) ? value.color : colors[0];
    return { variant, color, colors };
}

/** Колонке нужна первая настройка: её не подключили, или про неё есть что спросить у человека (часы, цвет). */
export function needsSetup(station: Station): boolean {
    if (station.status === 'pending') return true;
    const look = station.look;
    if (station.status !== 'approved' || !look) return false;
    return (
        (station.model == null && look.variants.length > 1) ||
        (station.color == null && look.colorFrom === 'default' && look.colors.length > 1)
    );
}

/** «Лайт 2 с часами · синяя». */
export function lookName(look: StationLook | null | undefined, drawing: StationDrawing): string {
    if (!look) return drawing.title;
    const variant = look.variants.find((entry) => entry.id === look.model);
    return `${variant?.title ?? look.title} · ${colorInfo(look.color).name}`;
}

const source = (key: LookSource): string => ({ station: tr.stationsScreen.sourceStation, chosen: tr.stationsScreen.sourceChosen, default: tr.stationsScreen.sourceDefault })[key];

/** «модель выбрана вами, цвет по данным колонки» — с родом каждого слова. */
function howOf(model: LookSource, color: LookSource): string {
    if (model === color) return source(model);
    const part = (from: LookSource, chosen: string): string => (from === 'chosen' ? chosen : source(from));
    return tr.stationsScreen.howMixed(part(model, tr.stationsScreen.modelChosen), part(color, tr.stationsScreen.colorChosen));
}

/** Что сказать под выбором: что колонка сообщила сама, что выбрано и что остаётся выбрать. */
function noteOf(look: StationLook, model: LookSource, color: LookSource): string {
    if (model === 'station' && color === 'station') return tr.stationsScreen.noteBothStation;
    if (color === 'station')
        return model === 'chosen'
            ? tr.stationsScreen.noteColorStationChosen
            : tr.stationsScreen.noteColorStation(look.variants[0].label.toLowerCase());
    if (model === 'station') return tr.stationsScreen.noteModelStation;
    if (model === 'chosen' || color === 'chosen') return tr.stationsScreen.noteChosen(look.title);
    return tr.stationsScreen.noteDefault(look.title);
}

/** Переключатель колонок в шапке: значок её цвета, имя и состояние. */
export function StationButton({
    name,
    drawing,
    status,
    reach,
    open,
    onOpen,
}: {
    name: string;
    drawing: StationDrawing;
    status: string;
    reach: Reach;
    open: boolean;
    onOpen: () => void;
}) {
    const { Glyph } = drawing;
    return (
        <button
            class="station-switch"
            aria-haspopup="dialog"
            aria-expanded={open}
            aria-label={tr.stationsScreen.switchLabel(name, status)}
            onClick={onOpen}
        >
            <span class="icon-btn glass" aria-hidden="true">
                <Glyph color={drawing.color} size={28} />
            </span>
            <span class="station-switch-text">
                <span class="station-switch-name">
                    <span>{name}</span>
                    <IconChevronDown />
                </span>
                <span class="t-meta flex items-center gap-6">
                    <span aria-hidden="true" class={dotClass(reach)} />
                    {status}
                </span>
            </span>
        </button>
    );
}

/**
 * Выбор вида: рисунок, модель (если данные оставили выбор) и цвет. Меняет одно за раз: `onChange` получает
 * только то, что выбрали, — окно «Колонки» сохраняет сразу, мастер копит до «Готово».
 */
export function LookEditor({
    station,
    value,
    disabled,
    onChange,
}: {
    station: Station;
    value: LookValue;
    disabled: boolean;
    onChange: (change: Partial<LookValue>) => void;
}) {
    const look = station.look;
    const drawing = drawingOf(look, station.deviceModel);
    const { Art } = drawing;
    if (!look)
        return (
            <div class="look-row">
                <div class="look-preview">
                    <Art skin={drawing.color} clock={drawing.clock} sticker={drawing.sticker} ring="var(--bar-own)" stage="idle" />
                </div>
                <p class="t-caption m-0 flex-1">
                    {station.deviceModel
                        ? tr.stationsScreen.noDrawing
                        : tr.stationsScreen.modelUnknown}
                </p>
            </div>
        );
    const { variant, color, colors } = previewLook(look, value);
    const sticker = value.sticker ?? look.sticker ?? false;
    const modelFrom: LookSource =
        look.variants.length === 1
            ? 'station'
            : value.model && look.variants.some((entry) => entry.id === value.model)
              ? 'chosen'
              : 'default';
    const colorFrom: LookSource =
        look.colorFrom === 'station' ? 'station' : value.color && colors.includes(value.color) ? 'chosen' : 'default';
    return (
        <>
            <div class="look-row">
                <div class="look-preview">
                    <Art skin={color} clock={variant.clock} sticker={sticker} ring="var(--bar-own)" stage="idle" />
                </div>
                <div class="look-side">
                    <span class="t-caption">{tr.stationsScreen.model}</span>
                    {look.variants.length > 1 ? (
                        <div role="radiogroup" aria-label={tr.stationsScreen.modelLabel} class="look-seg glass">
                            {look.variants.map((entry) => (
                                <button
                                    key={entry.id}
                                    role="radio"
                                    aria-checked={entry.id === variant.id}
                                    disabled={disabled}
                                    onClick={() => entry.id !== variant.id && onChange({ model: entry.id })}
                                >
                                    {entry.label}
                                </button>
                            ))}
                        </div>
                    ) : (
                        <span class="pill pill-off self-start">
                            {variant.label}
                        </span>
                    )}
                    <span class="look-name">
                        <b>
                            {variant.title}, {colorInfo(color).name}
                        </b>
                        <span class="t-caption">{howOf(modelFrom, colorFrom)}</span>
                    </span>
                </div>
            </div>
            <div class="look-colors">
                <span class="t-caption">{tr.stationsScreen.color}</span>
                <div role="radiogroup" aria-label={tr.stationsScreen.colorLabel} class="look-swatches">
                    {colors.map((id, index) => {
                        const info = colorInfo(id);
                        const on = id === color;
                        return (
                            <button
                                key={id}
                                role="radio"
                                aria-checked={on}
                                aria-label={tr.stationsScreen.colorLabelOf(info.label, look.colorFrom === 'station', index === 0)}
                                disabled={disabled || colors.length === 1}
                                class={`look-swatch${on ? ' on' : ''}`}
                                style={{ background: `linear-gradient(160deg, ${info.top} 0%, ${info.body} 75%)` }}
                                onClick={() => !on && onChange({ color: id })}
                            >
                                {on && <IconCheck size={16} />}
                            </button>
                        );
                    })}
                </div>
            </div>
            {look.sticker != null && (
                <div class="look-sticker">
                    <span class="look-name">
                        <b>{tr.stationsScreen.sticker}</b>
                        <span class="t-caption">{tr.stationsScreen.stickerAbout}</span>
                    </span>
                    <Switch on={sticker} label={tr.stationsScreen.stickerLabel} disabled={disabled} onChange={(on) => onChange({ sticker: on })} />
                </div>
            )}
            <p class="t-caption m-0">
                {noteOf(look, modelFrom, colorFrom)}
            </p>
        </>
    );
}

interface PickerProps {
    /** null — список не прочитать (нет права или шлюз постарше). */
    stations: Station[] | null;
    current: Station | null;
    sessions: SessionSnapshot[] | null;
    /** Присутствие колонок из живого канала: на месте ли колонка, даже без голосового канала. */
    presence?: Record<string, StationPresence>;
    onPick: (device: string) => void;
    onSetup: (device: string) => void;
    onChanged: () => void;
    onClose: () => void;
    /** «Мозг» из мастера подключения: своя модель вместо ответов Яндекса. */
    onOpenBrain?: () => void;
}

/** Окно «Колонки» из макета: какие есть и какая в сети, «Добавить колонку» и вид выбранной. */
export function StationsPicker({ stations, current, sessions, presence, onPick, onSetup, onChanged, onClose, onOpenBrain }: PickerProps) {
    const panel = useRef<HTMLElement>(null);
    const labelId = useId();
    const [connecting, setConnecting] = useState(false);
    const [restoring, setRestoring] = useState(false);
    const [busy, setBusy] = useState(false);
    const [error, setError] = useState<string | null>(null);
    useDialogFocus(panel, onClose);

    const visible = (stations ?? []).filter((station) => station.status !== 'blocked');
    const approved = visible.filter((station) => station.status === 'approved');
    const reach = (device: string): Reach =>
        reachOf(presence?.[device], sessions ? sessions.some((session) => session.deviceTag === device && !session.closedAt) : undefined);
    const mayManage = can('stations.manage');

    /** Вид меняется сразу, как в макете: шлюз решает, что из выбора принять. */
    const save = async (change: Partial<LookValue>): Promise<void> => {
        if (!current) return;
        setBusy(true);
        setError(null);
        try {
            await api.updateStation(current.device, change);
            onChanged();
        } catch (e) {
            setError(problemOf(e));
        } finally {
            setBusy(false);
        }
    };

    // Мастер подключения и откат — свои окна вместо списка: одно окно за раз.
    if (connecting)
        return (
            <ConnectSheet
                onClose={() => {
                    setConnecting(false);
                    onChanged();
                }}
                onConnected={(device) => {
                    onChanged();
                    onSetup(device);
                }}
                onOpenBrain={onOpenBrain}
            />
        );
    if (restoring && current?.lastAddress)
        return <RestoreSheet address={current.lastAddress} name={displayName(current, drawingOf(current.look, current.deviceModel))} onClose={() => setRestoring(false)} />;

    return (
        <div class="picker-layer">
            <button class="picker-scrim" aria-label={tr.stationsScreen.close} tabIndex={-1} onClick={onClose} />
            <section ref={panel} role="dialog" aria-modal="true" aria-labelledby={labelId} class="picker menu">
                <div class="picker-head">
                    <h2 id={labelId} class="t-title m-0">
                        {tr.stationsScreen.title}
                    </h2>
                    {stations && (
                        <span class="t-meta">{tr.stationsScreen.online(approved.filter((station) => isReachable(reach(station.device))).length, approved.length)}</span>
                    )}
                </div>
                {stations === null ? (
                    <p class="t-meta m-0">
                        {tr.stationsScreen.listForAdmins}
                    </p>
                ) : (
                    <div class="flex flex-col">
                        <div role="radiogroup" aria-labelledby={labelId} class="flex flex-col">
                            {visible.map((station) => {
                                const drawing = drawingOf(station.look, station.deviceModel);
                                const pending = station.status === 'pending';
                                const on = station.device === current?.device;
                                const { Glyph } = drawing;
                                return (
                                    <button
                                        key={station.device}
                                        role="radio"
                                        aria-checked={on}
                                        class="picker-row"
                                        onClick={() => (pending ? onSetup(station.device) : onPick(station.device))}
                                    >
                                        <span class="picker-glyph" aria-hidden="true">
                                            <Glyph color={drawing.color} size={26} />
                                        </span>
                                        <span class="dash-option-text">
                                            <b>{displayName(station, drawing)}</b>
                                            <span class="t-caption">
                                                {pending
                                                    ? mayManage
                                                        ? tr.stationsScreen.pendingManage
                                                        : tr.stationsScreen.pendingAdmin
                                                    : lookName(station.look, drawing)}
                                            </span>
                                        </span>
                                        <span
                                            aria-hidden="true"
                                            class={pending ? 'status-dot dot-unknown' : dotClass(reach(station.device))}
                                        />
                                        <span class={`dash-radio${on ? ' on' : ''}`} aria-hidden="true">
                                            {on && <IconCheck />}
                                        </span>
                                    </button>
                                );
                            })}
                            {!visible.length && (
                                <p class="t-meta m-0 py-8 px-2">
                                    {tr.stationsScreen.noStations}
                                </p>
                            )}
                        </div>
                        {mayManage && !here('stations.connect') && (
                            <p class="t-caption m-0 py-8 px-2">
                                {tr.stationsScreen.onStationBefore}<code>duowave station point local --reboot</code>
                            </p>
                        )}
                        {mayManage && here('stations.connect') && (
                            <button class="picker-row picker-add" onClick={() => setConnecting(true)}>
                                <span class="picker-add-icon" aria-hidden="true">
                                    <IconPlus />
                                </span>
                                <span class="dash-option-text">
                                    <b class="c-accent-text">{tr.stationsScreen.addTitle}</b>
                                    <span class="t-caption">{tr.stationsScreen.addAbout}</span>
                                </span>
                            </button>
                        )}
                    </div>
                )}
                {current && (
                    <>
                        <div class="divider" />
                        <LookEditor
                            station={current}
                            value={{ model: current.model ?? null, color: current.color ?? null, sticker: current.look?.sticker ?? null }}
                            disabled={!mayManage || busy}
                            onChange={(change) => void save(change)}
                        />
                        {!mayManage && (
                            <p class="t-caption m-0">
                                {tr.stationsScreen.onlyAdminLook}
                            </p>
                        )}
                        {current.firmware && (
                            <p class="t-caption m-0">
                                {tr.stationsScreen.firmware(current.firmware)}
                                {current.firmwareTried === true && tr.stationsScreen.firmwareTried}
                                {current.firmwareTried === false &&
                                    tr.stationsScreen.firmwareUntried}
                            </p>
                        )}
                        {mayManage && here('stations.connect') && current.status === 'approved' && current.lastAddress && (
                            <button class="pill pill-off dash-wide" onClick={() => setRestoring(true)}>
                                {tr.stationsScreen.restoreYandex}
                            </button>
                        )}
                    </>
                )}
                {error && (
                    <p role="alert" class="t-meta m-0 c-dot-offline">
                        {error}
                    </p>
                )}
                <button class="dash-done" onClick={onClose}>
                    {tr.stationsScreen.done}
                </button>
            </section>
        </div>
    );
}

/**
 * Первая настройка колонки: подключить (если ждёт одобрения), назвать и сказать, какая она, — только то, чего
 * шлюз не видит по её данным.
 */
export function SetupSheet({
    station,
    onClose,
    onChanged,
    onLater,
}: {
    station: Station;
    onClose: () => void;
    onChanged: () => void;
    onLater: () => void;
}) {
    const [step, setStep] = useState<'approve' | 'name' | 'look'>(station.status === 'pending' ? 'approve' : 'name');
    const [name, setName] = useState(station.name === DEFAULT_NAME ? '' : station.name);
    const [value, setValue] = useState<LookValue>({ model: station.model ?? null, color: station.color ?? null, sticker: station.look?.sticker ?? null });
    const [busy, setBusy] = useState(false);
    const [error, setError] = useState<string | null>(null);
    const mayManage = can('stations.manage');
    const drawing = drawingOf(station.look, station.deviceModel);
    const look = station.look;
    const askLook = !!look && (look.variants.length > 1 || (look.colorFrom !== 'station' && look.colors.length > 1) || look.sticker != null);
    const { Art } = drawing;

    const run = async (work: () => Promise<void>): Promise<void> => {
        setBusy(true);
        setError(null);
        try {
            await work();
        } catch (e) {
            setError(problemOf(e));
        } finally {
            setBusy(false);
        }
    };

    const approve = () =>
        run(async () => {
            await api.updateStation(station.device, { status: 'approved' });
            onChanged();
            setStep('name');
        });
    const block = () =>
        run(async () => {
            await api.updateStation(station.device, { status: 'blocked' });
            onChanged();
            onClose();
        });
    /** Имя и вид — одним изменением: то, что человек подтвердил «Готово», становится его выбором. */
    const finish = () =>
        run(async () => {
            const change: { name?: string; model?: string | null; color?: string | null; sticker?: boolean | null } = {};
            if (name.trim()) change.name = name.trim();
            if (look && askLook) {
                const preview = previewLook(look, value);
                if (look.variants.length > 1) change.model = preview.variant.id;
                if (look.colorFrom !== 'station' && preview.colors.length > 1) change.color = preview.color;
                if (look.sticker != null && value.sticker != null && value.sticker !== look.sticker) change.sticker = value.sticker;
            }
            if (Object.keys(change).length) await api.updateStation(station.device, change);
            onChanged();
            onClose();
        });

    const later = (
        <button class="pill pill-off dash-wide" disabled={busy} onClick={onLater}>
            {tr.stationsScreen.later}
        </button>
    );
    const problem = error && (
        <p role="alert" class="t-meta m-0 c-dot-offline">
            {error}
        </p>
    );

    if (step === 'approve')
        return (
            <Sheet
                title={tr.stationsScreen.newStation}
                about={`${lookName(look, drawing)}${station.lastAddress ? tr.stationsScreen.fromAddress(station.lastAddress) : ''}`}
                onClose={onClose}
                footer={
                    mayManage ? (
                        <div class="dash-actions">
                            <button class="dash-done" disabled={busy} onClick={() => void approve()}>
                                {busy ? tr.stationsScreen.connecting : tr.stationsScreen.connect}
                            </button>
                            <button class="pill pill-off dash-wide" disabled={busy} onClick={() => void block()}>
                                {tr.stationsScreen.dontConnect}
                            </button>
                        </div>
                    ) : undefined
                }
            >
                <div class="look-row">
                    <div class="look-preview">
                        <Art skin={drawing.color} clock={drawing.clock} sticker={drawing.sticker} ring="var(--bar-off)" stage="idle" offline />
                    </div>
                    <p class="t-meta m-0 flex-1 c-text">
                        {tr.stationsScreen.asksBefore}
                        {station.firstSeen ? tr.stationsScreen.firstSeen(timeAgo(station.firstSeen)) : ''}
                        {tr.stationsScreen.asksAfter}
                    </p>
                </div>
                {!mayManage && (
                    <p class="t-caption m-0">
                        {tr.stationsScreen.onlyAdminConnect}
                    </p>
                )}
                {problem}
            </Sheet>
        );

    if (step === 'name')
        return (
            <Sheet
                title={tr.stationsScreen.nameTitle}
                about={tr.stationsScreen.nameAbout}
                onClose={onClose}
                footer={
                    <div class="dash-actions">
                        <button class="dash-done" disabled={busy} onClick={() => (askLook ? setStep('look') : void finish())}>
                            {askLook ? tr.stationsScreen.next : busy ? tr.stationsScreen.saving : tr.stationsScreen.done}
                        </button>
                        {later}
                    </div>
                }
            >
                <input
                    class="field"
                    value={name}
                    maxLength={40}
                    placeholder={tr.stationsScreen.namePlaceholder}
                    aria-label={tr.stationsScreen.nameLabel}
                    onInput={(event) => setName((event.target as HTMLInputElement).value)}
                    onKeyDown={(event) => {
                        if (event.key === 'Enter') askLook ? setStep('look') : void finish();
                    }}
                />
                <div class="name-chips" role="group" aria-label={tr.stationsScreen.readyNames}>
                    {tr.stationsScreen.places.map((place) => (
                        <button
                            key={place}
                            class={`pill ${name === place ? 'pill-own' : 'pill-off'} dash-chip`}
                            aria-pressed={name === place}
                            onClick={() => setName(place)}
                        >
                            {place}
                        </button>
                    ))}
                </div>
                {problem}
            </Sheet>
        );

    return (
        <Sheet
            title={tr.stationsScreen.lookTitle}
            about={tr.stationsScreen.lookAbout}
            onClose={onClose}
            footer={
                <div class="dash-actions">
                    <button class="dash-done" disabled={busy} onClick={() => void finish()}>
                        {busy ? tr.stationsScreen.saving : tr.stationsScreen.done}
                    </button>
                    <button class="pill pill-off dash-wide" disabled={busy} onClick={() => setStep('name')}>
                        {tr.stationsScreen.back}
                    </button>
                </div>
            }
        >
            <LookEditor station={station} value={value} disabled={busy} onChange={(change) => setValue({ ...value, ...change })} />
            {problem}
        </Sheet>
    );
}

/** Плашка на дашборде: новая колонка ждёт одобрения или про неё есть что спросить; «Позже» — в мастере. */
export function SetupBanner({ station, onOpen }: { station: Station; onOpen: () => void }) {
    const drawing = drawingOf(station.look, station.deviceModel);
    const pending = station.status === 'pending';
    const mayManage = can('stations.manage');
    const { Glyph } = drawing;
    const unknown = station.look && station.look.colorFrom === 'default' ? tr.stationsScreen.unknownBoth : tr.stationsScreen.unknownClock;
    return (
        <section class="card glass setup-banner" aria-label={pending ? tr.stationsScreen.newStation : tr.stationsScreen.setupLabel}>
            <span class="picker-glyph" aria-hidden="true">
                <Glyph color={drawing.color} size={26} />
            </span>
            <div class="flex-1 min-w-0 flex flex-col">
                <b class="fs-14 lh-20">{pending ? tr.stationsScreen.newStation : tr.stationsScreen.whichYours(drawing.title)}</b>
                <span class="t-caption">
                    {pending
                        ? mayManage
                            ? `${tr.stationsScreen.asksConnect}${station.lastAddress ? ` · ${station.lastAddress}` : ''}`
                            : tr.stationsScreen.waitingAdmin
                        : tr.stationsScreen.gatewayCantSee(unknown)}
                </span>
            </div>
            {mayManage && (
                <button class="pill pill-own setup-go" onClick={onOpen}>
                    {pending ? tr.stationsScreen.look : tr.stationsScreen.setUp}
                </button>
            )}
        </section>
    );
}
