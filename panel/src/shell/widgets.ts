// Виджеты стола (docs/plans/desk.md): вклады в слот `desk.widgets` — встроенные (builtin.tsx) и из плагинов — и
// карточки `home.cards`, которые встают на стол большими, в своей рамке. Что стоит на столе, где и какой ширины, вид
// поля и закреплён ли стол — стол человека, свой у каждой роли (шлюз, помощник). Его хранит шлюз
// (`/api/v1/desk/<роль>`, gateway/src/panel/desk.rs): другой браузер и другой компьютер открывают тот же стол. Этот
// браузер держит его копию (`desk-layout-<роль>`): стол открывается сразу и живёт и без шлюза — без связи, у ключа API,
// в окне колонки (via.ts). Ничего не хранится — стартовый стол: встроенные виджеты по порядку вкладов.

import { useEffect, useState } from 'preact/hooks';

import { ApiError, api, sessionStore } from '../api';
import type { Health, KeptDesk } from '../api';
import { VIA, gw } from '../via';
import type { Role } from './role';
import { PlacedContribution, useSlot } from './slot';

export type WidgetSize = 'small' | 'large';

export interface DeskWidget extends PlacedContribution {
    size: WidgetSize;
    /** Стоит на столе сразу, пока человек его не убрал. */
    initial: boolean;
}

/** Место виджета на свободном столе: от колонки в середине, в точках стола; ширина и высота — если растягивали. */
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
    /** Места на свободном столе: переставленные руками. */
    places: Record<string, WidgetPlace>;
    /** Порядок в ленте телефона и в столбце боковой панели: переставленные руками. */
    order: string[];
    /** Масштаб стола и его сдвиг. */
    zoom?: number;
    pan?: { x: number; y: number };
    /** Стол закреплён: свободный стол не сдвигается и не масштабируется, виджеты не двигаются и не тянутся. */
    locked?: boolean;
    /**
     * Только что сели на стол из «Примерки» (docs/plans/voice-creation.md, «Посадка»): виджет → когда, мс от эпохи. Сутки
     * у такого виджета метка «новый».
     */
    fresh?: Record<string, number>;
}

/** Метка «новый» у севшего на стол виджета держится сутки. */
export const FRESH_MS = 24 * 60 * 60 * 1000;

/** Новый ли виджет: сел на стол меньше суток назад. */
export function isFresh(layout: DeskLayout, key: string, now = Date.now()): boolean {
    const at = layout.fresh?.[key];
    return at !== undefined && now - at < FRESH_MS;
}

const EMPTY: DeskLayout = { v: 1, hidden: [], added: [], places: {}, order: [] };
const layoutKey = (role: Role): string => `desk-layout-${role}`;
const markKey = (role: Role): string => `desk-kept-${role}`;
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
        locked: value.locked === true || undefined,
        fresh: Object.keys(fresh).length ? fresh : undefined,
    };
}

/** Стартовый стол: ничего не убрано, не поставлено и не переставлено, вид — как сначала, не закреплён. */
function starting(layout: DeskLayout): boolean {
    return (
        layout.hidden.length === 0 &&
        layout.added.length === 0 &&
        Object.keys(layout.places).length === 0 &&
        layout.order.length === 0 &&
        layout.zoom === undefined &&
        layout.pan === undefined &&
        !layout.locked
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

/** Сверка для этого входа; null — стол живёт только в этом браузере (окно колонки у помощника, вход не выполнен). */
function syncOf(role: Role): Sync | null {
    if (VIA) return null;
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

/** Правка стола: сразу — на экран и в копию браузера, потом — на шлюз. */
function update(role: Role, change: (layout: DeskLayout) => DeskLayout, delay = SAVE_AFTER): void {
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

/** Стоит ли виджет на столе. */
export function onDesk(widget: DeskWidget, layout: DeskLayout): boolean {
    if (layout.hidden.includes(widget.key)) return false;
    return widget.initial || layout.added.includes(widget.key);
}

export interface DeskActions {
    /** Убрать со стола (из «+» его можно поставить снова). */
    hide: (id: string) => void;
    /** Поставить на стол из «+». */
    add: (id: string) => void;
    /** Место на свободном столе. */
    place: (id: string, place: WidgetPlace) => void;
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
}

export function deskActions(role: Role): DeskActions {
    const change = (next: (layout: DeskLayout) => DeskLayout, delay?: number) => update(role, next, delay);
    return {
        hide: (id) =>
            change((layout) => ({
                ...layout,
                hidden: [...layout.hidden.filter((entry) => entry !== id), id],
                added: layout.added.filter((entry) => entry !== id),
            })),
        add: (id) =>
            change((layout) => ({
                ...layout,
                hidden: layout.hidden.filter((entry) => entry !== id),
                added: [...layout.added.filter((entry) => entry !== id), id],
            })),
        place: (id, place) => change((layout) => ({ ...layout, places: { ...layout.places, [id]: place } })),
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
        lock: (on) => change((layout) => ({ ...layout, locked: on || undefined }), 0),
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
    const rank = (widget: DeskWidget): number => {
        const at = layout.order.indexOf(widget.key);
        return at >= 0 ? at : layout.order.length + shown.indexOf(widget);
    };
    return [...shown].sort((a, b) => rank(a) - rank(b));
}
