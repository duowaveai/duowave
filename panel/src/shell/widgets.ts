// Виджеты стола (docs/plans/desk.md): вклады в слот `desk.widgets` — встроенные (builtin.tsx) и из плагинов — и
// карточки `home.cards`, которые встают на стол большими, в своей рамке. Что стоит на столе, где и какой ширины, вид
// поля и закреплён ли стол — стол человека, свой у каждой роли (шлюз, помощник). Его хранит шлюз
// (`/api/v1/desk/<роль>`, gateway/src/panel/desk.rs): другой браузер и другой компьютер открывают тот же стол. Стол
// дома — у центра (docs/plans/home-and-devices.md, этап 2): окно дома на компьютере (`?via=`) пишет его центру через
// проход, под id человека, — на компьютере и на телефоне стол один. Браузер держит только копию для первой отрисовки
// (`desk-layout-<роль>[@<центр>]`); без связи и у ключа API стол живёт в ней. Ничего не хранится — стартовый стол.

import { useEffect, useState } from 'preact/hooks';

import { ApiError, api, sessionStore } from '../api';
import type { Health, KeptDesk } from '../api';
import { VIA, gw } from '../via';
import { repairedLayout } from './desk-repair';
import type { Role } from './role';
import { PlacedContribution, useSlot } from './slot';

export type WidgetSize = 'small' | 'large';

export interface DeskWidget extends PlacedContribution {
    size: WidgetSize;
    /** Стоял сразу на прежнем стартовом столе: так и стоит у столов, которые меняли до нового набора (`oldDesk`). */
    initial: boolean;
}

/**
 * Место виджета на свободном столе: от колонки в середине, в точках стола; ширина — если растягивали. Высота — с какой
 * место заняли: спрятанный, пока пуст, виджет держит столько, и новый на его место не встаёт.
 */
export interface WidgetPlace {
    x: number;
    y: number;
    w?: number;
    h?: number;
}

export interface DeskLayout {
    v: 1;
    /** Убранные со стола (из тех, что стоят сразу). */
    hidden: string[];
    /** Поставленные из «+». */
    added: string[];
    /**
     * Места на свободном столе: переставленные руками и те, что стол дал сам, когда виджет появился (первое свободное,
     * screens/desk-places.ts), — дальше виджет держит своё. Убрали со стола — место освобождается.
     */
    places: Record<string, WidgetPlace>;
    /** Порядок в ленте телефона и в столбце боковой панели: переставленные руками. */
    order: string[];
    /** Масштаб стола и его сдвиг. */
    zoom?: number;
    pan?: { x: number; y: number };
    /**
     * Стол открыт для правки (`false`): свободный стол сдвигается и масштабируется, виджеты двигаются и тянутся. Нет поля —
     * стол закреплён: так он и начинается (владелец, 16.11), и в углу тогда только замок.
     */
    locked?: false;
    /**
     * Только что сели на стол из «Примерки» (docs/plans/voice-creation.md, «Посадка»): виджет → когда, мс от эпохи. Сутки
     * у такого виджета метка «новый».
     */
    fresh?: Record<string, number>;
    /**
     * Ярлыки на столе (docs/plans/menu-scheme.md): приложения, что открываются в одно нажатие, по порядку. Нет поля —
     * ни одного (владелец, 16.11). Раскладка хранится на шлюзе — ярлыки одни на всех устройствах человека.
     */
    shortcuts?: string[];
    /**
     * Лента ярлыков на столе включена (владелец, 16.11): по умолчанию её нет, включают только в «Настройках» →
     * «Оформление». Выключена — ни ленты, ни «+ Ярлык», ни голосовых «добавь ярлык»; список `shortcuts` сохраняется.
     */
    shortcutsBar?: boolean;
    /**
     * Приложения, чей ярлык стол уже ставил сам, когда их поставили (`SHORTCUT_WHEN_INSTALLED`): второй раз не ставит —
     * убранный человеком ярлык не возвращается.
     */
    offered?: string[];
    /**
     * Стол живёт по новому стартовому набору (docs/plans/start-desk.md, 16.11): сцена, витрина и живые виджеты. Нет
     * отметки у стола, который меняли раньше, — у него стоит всё, что стояло сразу тогда (`DeskWidget.initial`): убранное
     * не появляется, стоявшее не пропадает. Свежий и сброшенный стол — по новому набору; любая его правка ставит отметку.
     */
    start?: 2;
}

/** Ярлыки чистого стола: ни одного (владелец, 16.11; было — магазин и «Папки»). */
export const STARTING_SHORTCUTS: string[] = [];

/**
 * Приложения, у которых ярлык появляется на столе сам, как только они поставлены и включены, — один раз. Пусто
 * (владелец, 16.11): ярлыки ставит только человек; было — AirPlay.
 */
export const SHORTCUT_WHEN_INSTALLED: string[] = [];

/** Метка «новый» у севшего на стол виджета держится сутки. */
export const FRESH_MS = 24 * 60 * 60 * 1000;

/** Новый ли виджет: сел на стол меньше суток назад. */
export function isFresh(layout: DeskLayout, key: string, now = Date.now()): boolean {
    const at = layout.fresh?.[key];
    return at !== undefined && now - at < FRESH_MS;
}

const EMPTY: DeskLayout = { v: 1, hidden: [], added: [], places: {}, order: [] };
const layoutKey = (role: Role): string => `desk-layout-${role}${VIA ? `@${VIA}` : ''}`;
const markKey = (role: Role): string => `desk-kept-${role}${VIA ? `@${VIA}` : ''}`;
/** Правки подряд (тащат виджет, листают поле) уходят на шлюз одной записью — когда человек остановился. */
const SAVE_AFTER = 600;

function stored(key: string): unknown {
    try {
        return JSON.parse(localStorage.getItem(key) ?? 'null');
    } catch {
        return null;
    }
}

function store(key: string, value: unknown): void {
    try {
        localStorage.setItem(key, JSON.stringify(value));
    } catch {
        // приватное окно: копия живёт до перезагрузки
    }
}

/** Раскладка из копии браузера или со шлюза: только то, что пульт понимает; чужое и битое — как нет её. */
function parse(raw: unknown): DeskLayout | null {
    if (!raw || typeof raw !== 'object') return null;
    const value = raw as Record<string, unknown>;
    if (value.v !== 1) return null;
    const finite = (entry: unknown): entry is number => typeof entry === 'number' && Number.isFinite(entry);
    const list = (entry: unknown): string[] => (Array.isArray(entry) ? entry.filter((item): item is string => typeof item === 'string') : []);
    const places: Record<string, WidgetPlace> = {};
    if (value.places && typeof value.places === 'object')
        for (const [id, place] of Object.entries(value.places as Record<string, Partial<Record<keyof WidgetPlace, unknown>> | null>))
            if (place && finite(place.x) && finite(place.y))
                places[id] = { x: place.x, y: place.y, w: finite(place.w) ? place.w : undefined, h: finite(place.h) ? place.h : undefined };
    const pan = value.pan as { x?: unknown; y?: unknown } | null | undefined;
    const fresh: Record<string, number> = {};
    if (value.fresh && typeof value.fresh === 'object')
        for (const [id, at] of Object.entries(value.fresh as Record<string, unknown>)) if (finite(at) && Date.now() - at < FRESH_MS) fresh[id] = at;
    return {
        v: 1,
        hidden: list(value.hidden),
        added: list(value.added),
        places,
        order: list(value.order),
        zoom: finite(value.zoom) && value.zoom >= 0.4 && value.zoom <= 2 ? value.zoom : undefined,
        pan: pan && finite(pan.x) && finite(pan.y) ? { x: pan.x, y: pan.y } : undefined,
        // Закреплён — как сначала, поэтому хранится только «открыт»; прежнее `true` — то же, что нет поля.
        locked: value.locked === false ? false : undefined,
        fresh: Object.keys(fresh).length ? fresh : undefined,
        shortcuts: Array.isArray(value.shortcuts) ? [...new Set(list(value.shortcuts))] : undefined,
        offered: Array.isArray(value.offered) ? [...new Set(list(value.offered))] : undefined,
        shortcutsBar: value.shortcutsBar === true || undefined,
        start: value.start === 2 ? 2 : undefined,
    };
}

/**
 * Стол по прежнему стартовому набору: его меняли до нового (docs/plans/start-desk.md) — на нём нет отметки `start: 2`.
 * Нетронутый стол — по новому набору, с отметкой или без.
 */
export function oldDesk(layout: DeskLayout): boolean {
    return layout.start !== 2 && !starting(layout);
}

/** Стартовый стол: ничего не убрано, не поставлено и не переставлено, вид — как сначала, закреплён. Отметка набора не в счёт. */
function starting(layout: DeskLayout): boolean {
    return (
        layout.hidden.length === 0 &&
        layout.added.length === 0 &&
        Object.keys(layout.places).length === 0 &&
        layout.order.length === 0 &&
        layout.zoom === undefined &&
        layout.pan === undefined &&
        layout.locked === undefined &&
        layout.shortcuts === undefined &&
        layout.offered === undefined &&
        !layout.shortcutsBar
    );
}

const same = (a: DeskLayout, b: DeskLayout): boolean => a === b || JSON.stringify(a) === JSON.stringify(b);

const layouts = new Map<Role, DeskLayout>();
const listeners = new Set<() => void>();

export function deskLayout(role: Role): DeskLayout {
    let layout = layouts.get(role);
    if (!layout) {
        layout = parse(stored(layoutKey(role))) ?? { ...EMPTY };
        layouts.set(role, layout);
    }
    return layout;
}

/** Раскладку — на экран и в копию браузера. */
function show(role: Role, next: DeskLayout): void {
    layouts.set(role, next);
    store(layoutKey(role), next);
    for (const listener of [...listeners]) listener();
}

// ─── Стол на шлюзе ───────────────────────────────────────────────

/** Как копия этого браузера сверена со шлюзом (тоже в браузере: правка, не дошедшая до шлюза, переживает перезагрузку). */
interface Mark {
    /** Время записи на шлюзе, с которой копия сверена; null — там ничего не хранится. */
    at: string | null;
    /** Есть правка, которую шлюз ещё не подтвердил. */
    pending: boolean;
    /** Кто её сделал (имя вошедшего): правка одного человека не уходит на стол другого. */
    who: string | null;
    /** Этот браузер хоть раз читал стол роли со шлюза. До того его копия — стол, что жил только в браузере. */
    seen: boolean;
}

const marks = new Map<Role, Mark>();

function markOf(role: Role): Mark {
    let mark = marks.get(role);
    if (!mark) {
        const raw = stored(markKey(role)) as Partial<Mark> | null;
        mark = {
            at: typeof raw?.at === 'string' ? raw.at : null,
            pending: raw?.pending === true,
            who: typeof raw?.who === 'string' ? raw.who : null,
            seen: raw?.seen === true,
        };
        marks.set(role, mark);
    }
    return mark;
}

function setMark(role: Role, mark: Mark): void {
    marks.set(role, mark);
    store(markKey(role), mark);
}

const whoAmI = (): string | null => sessionStore.get()?.name ?? null;

/** Сверка стола роли со шлюзом — на один вход: другой вход — другой человек и его стол. */
interface Sync {
    token: string;
    /** Стол прочитан со шлюза: до того правки копятся в `queued` и лягут поверх прочитанного. */
    read: boolean;
    /** Стол читают (при входе, открыли «+», вкладку снова показали): правки копятся в `queued`, записи ждут сверки. */
    reading: boolean;
    writing: boolean;
    /** Шлюз стол не держит (ключ API — 403, шлюз постарше — 404, вход кончился — 401): стол — только в этом браузере. */
    refused: boolean;
    /**
     * Правки, сделанные, пока стол читали. Они уже на экране и в копии; если выиграет стол со шлюза (его поменяли в
     * другом браузере), они лягут поверх него и уйдут на шлюз.
     */
    queued: Array<(layout: DeskLayout) => DeskLayout>;
    /** Отложенная запись или повтор после сбоя. */
    timer?: number;
    /** Пауза до повтора, когда шлюз не ответил, мс. */
    backoff: number;
    /** Раскладка, уже отправленная при уходе со страницы: второй раз — не нужно. */
    flushed?: DeskLayout;
    /** Сколько ответов записей учтено в отметке: по нему видно, что ответ пришёл не по порядку (wrote). */
    answers: number;
}

const syncs = new Map<Role, Sync>();

/** Сверка для этого входа; null — стол живёт только в этом браузере (вход не выполнен). */
function syncOf(role: Role): Sync | null {
    const token = sessionStore.get()?.token;
    if (!token) return null;
    let sync = syncs.get(role);
    if (!sync || sync.token !== token) {
        if (sync) window.clearTimeout(sync.timer);
        sync = { token, read: false, reading: false, writing: false, refused: false, queued: [], backoff: 0, answers: 0 };
        syncs.set(role, sync);
    }
    return sync;
}

const live = (role: Role, sync: Sync): boolean => syncs.get(role) === sync;

/**
 * Стол роли со шлюза: при входе, при смене роли и когда вкладку снова показали. Пока своя правка не дошла до шлюза,
 * перечитывать нечего — своя новее.
 */
function pull(role: Role): void {
    const sync = syncOf(role);
    if (!sync || sync.refused || sync.reading) return;
    if (sync.read && (sync.writing || sync.timer !== undefined || markOf(role).pending)) return;
    window.clearTimeout(sync.timer);
    sync.timer = undefined;
    sync.reading = true;
    api.desk<unknown>(role).then(
        (kept) => {
            if (!live(role, sync)) return;
            sync.reading = false;
            sync.read = true;
            sync.backoff = 0;
            settle(role, sync, kept);
        },
        (error: unknown) => {
            if (!live(role, sync)) return;
            sync.reading = false;
            failed(role, sync, error);
        },
    );
}

/** Стол со шлюза пришёл: чей новее — свой, не дошедший до шлюза, или тот, что там. */
function settle(role: Role, sync: Sync, kept: KeptDesk<unknown>): void {
    const local = deskLayout(role);
    const mark = markOf(role);
    const theirs = parse(kept.layout);
    const queued = sync.queued.splice(0);
    let next: DeskLayout;
    let send: boolean;
    if (mark.pending && mark.seen && mark.who === whoAmI() && kept.at === mark.at) {
        // Своя правка, которую шлюз не видел, а там с прошлой сверки ничего не меняли: своя новее, она и уходит.
        next = local;
        send = true;
    } else if (!theirs && !mark.seen && !starting(local)) {
        // Шлюз стола ещё не держит, а этот браузер свой помнит (так стол жил до шлюза): он уходит туда — один раз.
        next = local;
        send = true;
    } else {
        // Стол со шлюза, а нет там ничего — стартовый; правки, сделанные, пока читали, — поверх, и уходит то, чем стол
        // теперь от шлюзового отличается.
        const base = theirs ?? { ...EMPTY };
        next = queued.reduce((layout, change) => change(layout), base);
        send = !same(next, base);
    }
    setMark(role, { at: kept.at, pending: send, who: whoAmI(), seen: true });
    if (!same(next, local)) show(role, next);
    if (send) schedule(role, 0);
}

/** Записать стол на шлюз через `delay` мс: правки подряд уходят одной записью. */
function schedule(role: Role, delay = SAVE_AFTER): void {
    const sync = syncOf(role);
    if (!sync || sync.refused || !sync.read) return;
    window.clearTimeout(sync.timer);
    sync.timer = window.setTimeout(() => {
        sync.timer = undefined;
        void push(role, sync);
    }, delay);
}

/**
 * Запись дошла до шлюза: отметка — по ней. Пока писали, стол могли поменять — тогда правка ещё ждёт своей записи.
 * Обычная запись и запрос с keepalive (flush) могут идти разом и ответить не по порядку. Ответ записи, которую шлюз
 * сделал раньше уже учтённой, отметку не трогает: иначе отметка отстала бы от шлюза, и правка, не дошедшая до него,
 * при перезагрузке проиграла бы столу, который никто не менял. Такой ответ — только тот, после чьей отправки уже учтён
 * ответ другой записи (`before`), и чьё время старше отметки. Одно время не решает: часы шлюза могут отставать от
 * времени, которое он поставил раньше (часы шагнули назад, колонка ещё не сверила их после загрузки), — и тогда каждый
 * ответ казался бы старым, а запись уходила бы снова и снова.
 */
function wrote(role: Role, sync: Sync, before: number, at: string | null, layout: DeskLayout): void {
    const mark = markOf(role);
    if (sync.answers !== before && at !== null && mark.at !== null && Date.parse(at) < Date.parse(mark.at)) return;
    sync.answers += 1;
    setMark(role, { ...mark, at, pending: deskLayout(role) !== layout, seen: true });
}

/**
 * Стол — на шлюз целиком. Стартовый там не хранится: шлюз его забывает (api.resetDesk), и везде — стартовый. Пока стол
 * читают, запись ждёт: правки этого времени лягут поверх прочитанного, и запись уйдёт после сверки (settle).
 */
async function push(role: Role, sync: Sync): Promise<void> {
    if (!live(role, sync) || sync.refused || sync.reading || sync.writing || !markOf(role).pending) return;
    const layout = deskLayout(role);
    // Правки, что ждали прочитанного стола (а чтение сорвалось), уходят в нём же: накладывать их больше не на что.
    sync.queued.length = 0;
    sync.writing = true;
    const before = sync.answers;
    try {
        let at: string | null = null;
        if (starting(layout)) await api.resetDesk(role);
        else at = (await api.saveDesk(role, layout)).at;
        if (!live(role, sync)) return;
        sync.backoff = 0;
        wrote(role, sync, before, at, layout);
    } catch (error) {
        if (live(role, sync)) failed(role, sync, error);
        return;
    } finally {
        sync.writing = false;
    }
    if (markOf(role).pending && sync.timer === undefined) schedule(role);
}

/**
 * Шлюз не принял или не ответил. Отказ (4xx) — стол живёт в этом браузере, тихо; нет связи или шлюз занят — повтор
 * позже, всё дольше, а правка ждёт в копии браузера.
 */
function failed(role: Role, sync: Sync, error: unknown): void {
    const status = error instanceof ApiError ? error.status : 0;
    window.clearTimeout(sync.timer);
    sync.timer = undefined;
    if (status >= 400 && status < 500 && status !== 408 && status !== 429) {
        sync.refused = true;
        return;
    }
    sync.backoff = Math.min(sync.backoff ? sync.backoff * 2 : 5_000, 120_000);
    sync.timer = window.setTimeout(() => {
        sync.timer = undefined;
        if (!live(role, sync)) return;
        if (sync.read) void push(role, sync);
        else pull(role);
    }, sync.backoff);
}

/**
 * Вкладку скрыли или закрывают: правка, что ждёт, уходит сразу. api.saveDesk не умеет keepalive, а без него запрос
 * обрывается вместе со страницей, — поэтому здесь свой запрос, тем же входом.
 */
function flush(): void {
    for (const [role, sync] of syncs) {
        const layout = deskLayout(role);
        if (sync.refused || !sync.read || !markOf(role).pending || sync.flushed === layout) continue;
        window.clearTimeout(sync.timer);
        sync.timer = undefined;
        sync.flushed = layout;
        const remove = starting(layout);
        const before = sync.answers;
        void fetch(gw(`/api/v1/desk/${role}`), {
            method: remove ? 'DELETE' : 'PUT',
            keepalive: true,
            headers: { Accept: 'application/json', Authorization: `Bearer ${sync.token}`, ...(remove ? {} : { 'Content-Type': 'application/json' }) },
            body: remove ? undefined : JSON.stringify(layout),
        })
            .then(async (response) => {
                if (!response.ok || !live(role, sync)) return;
                const at = remove ? null : (((await response.json()) as { message?: { at?: string } | null }).message?.at ?? null);
                wrote(role, sync, before, at, layout);
            })
            .catch(() => undefined);
    }
}

/**
 * Вкладку снова показали или вернулась связь: своя правка, что ждёт, — на шлюз сейчас; нет её — стол со шлюза: его
 * могли поменять в другом браузере.
 */
function resume(role: Role): void {
    const sync = syncOf(role);
    if (!sync || sync.refused) return;
    sync.backoff = 0;
    if (sync.read && markOf(role).pending) {
        if (!sync.writing) schedule(role, 0);
        return;
    }
    pull(role);
}

if (typeof window !== 'undefined') {
    document.addEventListener('visibilitychange', () => {
        if (document.visibilityState === 'hidden') flush();
        else for (const role of [...syncs.keys()]) resume(role);
    });
    window.addEventListener('pagehide', flush);
    window.addEventListener('online', () => {
        for (const role of [...syncs.keys()]) resume(role);
    });
}

/**
 * Правка стола: сразу — на экран и в копию браузера, потом — на шлюз. Правка стола по новому стартовому набору ставит
 * ему отметку `start: 2`: иначе изменённый стол без неё жил бы по прежнему набору.
 */
function update(role: Role, edit: (layout: DeskLayout) => DeskLayout, delay = SAVE_AFTER): void {
    const change = (layout: DeskLayout): DeskLayout => {
        const next = edit(layout);
        return next.start === 2 || oldDesk(layout) || same(next, layout) ? next : { ...next, start: 2 };
    };
    const before = deskLayout(role);
    const next = change(before);
    if (same(next, before)) return;
    show(role, next);
    const sync = syncOf(role);
    if (!sync || sync.refused) return;
    setMark(role, { ...markOf(role), pending: true, who: whoAmI() });
    if (sync.read && !sync.reading) {
        schedule(role, delay);
        return;
    }
    // Стол со шлюза ещё не прочитан (виджет поставили до того, как открылся стол) или его как раз перечитывают (открыли
    // «+», вкладку снова показали): правка ляжет поверх прочитанного и уйдёт после сверки — даже если стол на шлюзе
    // за это время поменяли в другом браузере.
    sync.queued.push(change);
    pull(role);
}

/**
 * Стоит ли виджет на столе: убранный — нет, поставленный — да; остальные — по стартовому набору стола: прежнему у
 * столов, которые меняли раньше, новому (только встроенные с `start`) — у остальных.
 */
export function onDesk(widget: DeskWidget, layout: DeskLayout): boolean {
    if (layout.hidden.includes(widget.key)) return false;
    if (layout.added.includes(widget.key)) return true;
    return oldDesk(layout) ? widget.initial : widget.plugin === 'core' && !!widget.start;
}

export interface DeskActions {
    /** Убрать со стола (из «+» его можно поставить снова). */
    hide: (id: string) => void;
    /** Поставить на стол из «+». */
    add: (id: string) => void;
    /** Место на свободном столе. */
    place: (id: string, place: WidgetPlace) => void;
    /** Места сразу у нескольких: стол дал новым места или отодвинул выросшим соседей. */
    places: (places: Record<string, WidgetPlace>) => void;
    /** Порядок в ленте: на шаг раньше (-1) или позже (+1) среди этих. */
    move: (id: string, step: -1 | 1, visible: string[]) => void;
    view: (zoom: number, pan: { x: number; y: number }) => void;
    /** Закрепить свободный стол или открепить. */
    lock: (on: boolean) => void;
    /**
     * Расставить заново: места, порядок и вид — как сначала, убранное и поставленное остаются. Остался стартовый стол —
     * шлюз хранимый забывает (api.resetDesk), иначе хранит остаток. На шлюз — сразу.
     */
    reset: () => void;
    /** Посадка из «Примерки»: на стол, на это место (на свободном столе), с меткой «новый» на сутки. На шлюз — сразу. */
    land: (id: string, place?: WidgetPlace) => void;
    /** Ярлыки стола — весь список по порядку (docs/plans/menu-scheme.md). На шлюз — сразу. */
    shortcuts: (list: string[]) => void;
    /** Включить ленту ярлыков на столе или выключить («Настройки» → «Оформление»). На шлюз — сразу. */
    shortcutsBar: (on: boolean) => void;
    /** Ярлык поставленного приложения из `SHORTCUT_WHEN_INSTALLED` — в конец списка, и отметка, что он уже был. На шлюз — сразу. */
    offer: (id: string) => void;
    /** Раскладка, починенная целиком (repairedLayout). На шлюз — сразу. */
    repair: (widgets: DeskWidget[]) => void;
}


export { repairedLayout };

export function deskActions(role: Role): DeskActions {
    const change = (next: (layout: DeskLayout) => DeskLayout, delay?: number) => update(role, next, delay);
    return {
        hide: (id) =>
            change((layout) => ({
                ...layout,
                hidden: [...layout.hidden.filter((entry) => entry !== id), id],
                added: layout.added.filter((entry) => entry !== id),
                places: Object.fromEntries(Object.entries(layout.places).filter(([key]) => key !== id)),
            })),
        add: (id) =>
            change((layout) => ({
                ...layout,
                hidden: layout.hidden.filter((entry) => entry !== id),
                added: [...layout.added.filter((entry) => entry !== id), id],
            })),
        place: (id, place) => change((layout) => ({ ...layout, places: { ...layout.places, [id]: place } })),
        places: (places) => change((layout) => ({ ...layout, places: { ...layout.places, ...places } })),
        move: (id, step, visible) =>
            change((layout) => {
                const order = [...visible];
                const at = order.indexOf(id);
                const to = at + step;
                if (at < 0 || to < 0 || to >= order.length) return layout;
                [order[at], order[to]] = [order[to], order[at]];
                return { ...layout, order };
            }),
        view: (zoom, pan) => change((layout) => ({ ...layout, zoom, pan })),
        lock: (on) => change((layout) => ({ ...layout, locked: on ? undefined : false }), 0),
        reset: () => change((layout) => ({ ...layout, places: {}, order: [], zoom: undefined, pan: undefined }), 0),
        land: (id, place) =>
            change(
                (layout) => ({
                    ...layout,
                    hidden: layout.hidden.filter((entry) => entry !== id),
                    added: [...layout.added.filter((entry) => entry !== id), id],
                    places: place ? { ...layout.places, [id]: place } : layout.places,
                    fresh: { ...Object.fromEntries(Object.entries(layout.fresh ?? {}).filter(([, at]) => Date.now() - at < FRESH_MS)), [id]: Date.now() },
                }),
                0,
            ),
        // Стартовый набор — снова «нет поля»: шлюз хранит только то, что поменяли.
        shortcuts: (list) =>
            change((layout) => {
                const unique = [...new Set(list)];
                const same = unique.length === STARTING_SHORTCUTS.length && unique.every((id, at) => id === STARTING_SHORTCUTS[at]);
                return { ...layout, shortcuts: same ? undefined : unique };
            }, 0),
        shortcutsBar: (on) => change((layout) => ({ ...layout, shortcutsBar: on || undefined }), 0),
        repair: (widgets) => change((layout) => repairedLayout(layout, widgets) ?? layout, 0),
        offer: (id) =>
            change((layout) => {
                if (layout.offered?.includes(id)) return layout;
                const list = layout.shortcuts ?? STARTING_SHORTCUTS;
                return {
                    ...layout,
                    shortcuts: list.includes(id) ? layout.shortcuts : [...list, id],
                    offered: [...(layout.offered ?? []), id],
                };
            }, 0),
    };
}

/** Раскладка стола роли — с перерисовкой, когда её меняют здесь, в «+» или она пришла со шлюза. */
export function useDeskLayout(role: Role): [DeskLayout, DeskActions] {
    const [, rerender] = useState(0);
    useEffect(() => {
        const update = () => rerender((n) => n + 1);
        listeners.add(update);
        return () => {
            listeners.delete(update);
        };
    }, []);
    // Стол человека со шлюза — когда пульт открылся и когда сменилась роль.
    useEffect(() => pull(role), [role]);
    return [deskLayout(role), deskActions(role)];
}

/**
 * Виджеты, которые видит этот вход в этой роли: `desk.widgets` и карточки `home.cards` (большие, в своей рамке).
 * Встроенные стоят на столе сразу; виджеты плагинов — если плагин так сказал, иначе их ставят из «+».
 */
export function useWidgets(role: Role, health: Health | null): DeskWidget[] {
    const desk = useSlot('desk.widgets');
    const cards = useSlot('home.cards');
    const widgets: DeskWidget[] = [
        ...desk.map((entry) => ({ ...entry, size: entry.size ?? 'small', initial: entry.initial ?? entry.plugin === 'core' })),
        ...cards.map((entry) => ({ ...entry, size: 'large' as const, initial: entry.initial ?? true, bare: entry.bare ?? true })),
    ];
    return widgets.filter((widget) => (!widget.roles || widget.roles.includes(role)) && (!widget.available || widget.available(health)));
}

/** Виджеты на столе по порядку: переставленные руками — как их поставили, остальные — по порядку вкладов. */
export function orderedOnDesk(widgets: DeskWidget[], layout: DeskLayout): DeskWidget[] {
    const shown = widgets.filter((widget) => onDesk(widget, layout));
    // Не переставленные руками — по порядку вкладов, но те, что прячутся, пока пусты (`hideWhileEmpty`), — после
    // остальных: появившись, такой встаёт в конец своего столбца или ленты и никого не сдвигает.
    const rank = (widget: DeskWidget): number => {
        const at = layout.order.indexOf(widget.key);
        return at >= 0 ? at : layout.order.length + (widget.hideWhileEmpty ? shown.length : 0) + shown.indexOf(widget);
    };
    return [...shown].sort((a, b) => rank(a) - rank(b));
}
