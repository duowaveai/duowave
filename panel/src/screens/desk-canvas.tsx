// Свободный стол (docs/plans/desk.md, этап 3; макеты SpaceGateway, SpaceHelper): поле без краёв, колонка — в начале
// координат, виджеты — где их поставили. Виджет перетаскивают за шапку и растягивают за угол; поле сдвигают
// перетаскиванием фона и колесом, масштаб — кнопками, ⌘/Ctrl + колесом (щипок на тачпаде); мини-карта показывает,
// что есть за краем, и ведёт туда. У каждого виджета своё место (desk-places.ts): новый встаёт на первое свободное у
// колонки — столбцами по сторонам — и дальше его держит. Колесо над списком в виджете, который ещё есть куда листать,
// листает его, а не поле. Над полем, у правого края, — то, что висит поверх стола и места на нём не берёт (DeskFloat:
// карточка Турбо).
// Замок в углу закрепляет раскладку: виджеты не двигаются и не тянутся, поле не масштабируется и не тащится мышью, пункты
// раскладки в меню виджетов не нажимаются. Смотреть стол колесом и тачпадом можно и у закреплённого (проверка начисто
// 18.11: витрина была видна наполовину, а вниз не листалось); такой сдвиг не хранится — открыли стол снова, он как
// сначала. Стол начинается закреплённым, и тогда в углу только замок; открыли — рядом появляются
// «Расставить заново», масштаб и мини-карта. Места, масштаб, сдвиг и замок — в раскладке стола: её хранит шлюз, свою у
// каждой роли (shell/widgets.ts). Плашки над столом (баннер: «Готовлю слух», флешка…) опускают поле, пока видны: сцена
// стоит под ними, а не за ними.

import { createContext } from 'preact';
import type { ComponentChildren } from 'preact';
import { useContext, useEffect, useLayoutEffect, useRef, useState } from 'preact/hooks';

import { IconGrid, IconLock, IconMinus, IconPlus, IconUnlock } from '../icons';
import { DeskActions, DeskLayout, DeskWidget, WidgetPlace } from '../shell/widgets';
import type { WidgetMenu } from '../ui/widget';
import { tr } from '../i18n';
import { Laid, Rect, arrange, pushApart, underTop } from './desk-places';

const MIN_ZOOM = 0.5;
const MAX_ZOOM = 1.6;
/** Ширина виджета: столбец — 300, растянуть можно от 240 до 640. */
const WIDTH = 300;
const MIN_W = 240;
const MAX_W = 640;
/** Сцена колонки: ширина и высота на столе. */
export const SCENE_W = 560;
export const SCENE_H = 520;
/** Широкий виджет под сценой (`wide`, витрина стола — docs/plans/start-desk.md): шире сцены, по её середине. */
const UNDER_WIDE = 908;
/**
 * Широкий виджет под сценой встаёт сразу под тем, что в сцене нарисовано (кольцо орбиты кончается выше низа сцены, под
 * ним бывают строки «Запомнил…» и «Поставить на стол?»): так он виден весь, без прокрутки, не заходит под док и не
 * ложится на эти строки. Обычный — ниже сцены, как раньше. На столько точек он заходит на пустой низ кольца.
 */
const UNDER_WIDE_OVERLAP = 16;
const GAP = 60;
/** Между столбцами виджетов одной стороны и между виджетом и витриной под сценой. */
const COLUMN_GAP = 40;
const ROW_GAP = 16;
/**
 * Сколько точек виджет занимает, пока его не измерили; и не меньше этого ему ищут место: только что открытый виджет
 * бывает ниже, чем станет (данные ещё идут), — в узкую щель он не встаёт, чтобы, дорастая, не толкать соседа.
 */
const GUESS_H = 200;
/** Мельче этого стол сам не уменьшается, чтобы сцена с витриной влезли над доком. */
const MIN_FIT = 0.7;
/** Между баннером и сценой под ним, точек экрана. */
const BANNER_GAP = 16;

interface Point {
    x: number;
    y: number;
}

/**
 * Место над полем у правого края — для того, что висит поверх стола (карточка Турбо): оно не на столе, не двигается с
 * полем и места у виджетов не берёт. Нет свободного стола (лента, столбец) — null: сцена рисует своё у себя.
 */
const DeskFloat = createContext<HTMLElement | null>(null);

export function useDeskFloat(): HTMLElement | null {
    return useContext(DeskFloat);
}

/** Сдвиг поля, пока его не трогали: колонка чуть выше середины окна. */
const START: Point = { x: 0, y: -40 };

const clamp = (value: number, min: number, max: number) => Math.min(max, Math.max(min, value));

/**
 * Вид поля, пока его не трогали, когда под колонкой стоит виджет (`under`: витрина, «Лента»): колонка поднимается к
 * верху поля (24 точки от него, не выше чем на 240), а если виджет под ней и тогда не влезает над доком — стол
 * уменьшается, но не мельче MIN_FIT. Так виджет под колонкой виден весь сразу: у закреплённого стола (как он
 * начинается) иначе до его низа не добраться было бы. `height` — высота поля, `room` — сколько точек экрана от верха
 * поля до дока, `head` — где может начаться сцена (ниже баннера), `need` — от верха сцены до низа виджета под ней,
 * точки стола.
 */
function startOf(withUnder: boolean, height: number, room: number, head: number, need: number): { pan: Point; zoom: number } {
    if (!withUnder || need <= 0) return { pan: START, zoom: 1 };
    const zoom = clamp(Math.floor(((room - head) / need) * 100) / 100, MIN_FIT, 1);
    return { pan: { x: 0, y: Math.round(Math.max(-240, 24 - height / 2 + (SCENE_H / 2) * zoom)) }, zoom };
}

/** Мерило открытого свободного стола: где элемент на нём (точка поля и ширина). Нет стола — null. */
let measure: ((element: Element) => WidgetPlace) | null = null;

/**
 * Место элемента на свободном столе — для посадки черновика на место его плитки (docs/plans/voice-creation.md,
 * «Посадка»). Нет свободного стола (лента, столбец) — null: виджет встаёт в ленту, на то же место.
 */
export function placeOnCanvas(element: Element | null): WidgetPlace | null {
    return element && measure ? measure(element) : null;
}

/** Под курсором — своё, что листается в эту сторону (список в виджете): колесо — ему, а не полю. */
function scrollsItself(from: HTMLElement, stop: HTMLElement, dx: number, dy: number): boolean {
    for (let node: HTMLElement | null = from; node && node !== stop; node = node.parentElement) {
        const style = getComputedStyle(node);
        if (dy && /auto|scroll/.test(style.overflowY) && (dy < 0 ? node.scrollTop > 0 : node.scrollTop + node.clientHeight < node.scrollHeight - 1)) return true;
        if (dx && /auto|scroll/.test(style.overflowX) && (dx < 0 ? node.scrollLeft > 0 : node.scrollLeft + node.clientWidth < node.scrollWidth - 1)) return true;
    }
    return false;
}

/** Перетаскивание: от нажатия до отпускания, шаги — в `move`. Отпустили — `done`. */
function drag(event: PointerEvent, move: (dx: number, dy: number) => void, done: (moved: boolean) => void): void {
    const start = { x: event.clientX, y: event.clientY };
    const target = event.currentTarget as HTMLElement | null;
    target?.setPointerCapture?.(event.pointerId);
    let moved = false;
    const onMove = (next: PointerEvent) => {
        const dx = next.clientX - start.x;
        const dy = next.clientY - start.y;
        if (!moved && Math.hypot(dx, dy) < 3) return;
        moved = true;
        move(dx, dy);
    };
    const onUp = () => {
        window.removeEventListener('pointermove', onMove);
        window.removeEventListener('pointerup', onUp);
        window.removeEventListener('pointercancel', onUp);
        done(moved);
    };
    window.addEventListener('pointermove', onMove);
    window.addEventListener('pointerup', onUp);
    window.addEventListener('pointercancel', onUp);
}

interface Props {
    widgets: DeskWidget[];
    layout: DeskLayout;
    actions: DeskActions;
    scene: ComponentChildren;
    /** «Новый виджет» — плитка в левом столбце. */
    addTile: ComponentChildren;
    /**
     * Сколько точек сцене нужно справа от колонки — для Турбо рядом со шлюзом (решение владельца 16.11): колонка остаётся
     * в середине поля, правые столбцы виджетов отходят на столько же.
     */
    sceneExtra?: number;
    banner: ComponentChildren;
    /** `grip` — перетаскивание за шапку; у закреплённого стола его нет. */
    render: (widget: DeskWidget, menu: WidgetMenu, grip?: (event: PointerEvent) => void) => ComponentChildren;
}

export function DeskCanvas({ widgets, layout, actions, scene, addTile, banner, render, sceneExtra = 0 }: Props) {
    const box = useRef<HTMLDivElement>(null);
    const plane = useRef<HTMLDivElement>(null);
    const top = -SCENE_H / 2;
    const withUnder = widgets.some((widget) => widget.under && !layout.places[widget.key]);
    /** С чего стол начинается, пока его не двигали: от него же «вид как сначала» не сохраняется. */
    const startPan = useRef<Point>(startOf(false, 0, 0, 0, 0).pan);
    const startZoom = useRef(1);
    const [zoom, setZoom] = useState(layout.zoom ?? startZoom.current);
    const [pan, setPan] = useState<Point>(layout.pan ?? startPan.current);
    /** Поле сдвинули в этот раз (колесом — и у закреплённого стола): вид как сначала его уже не возвращает. */
    const touched = useRef(false);
    /** Высоты виджетов и места «Новый виджет» (точки стола): по ним ищется свободное место. 0 — не виден. */
    const [heights, setHeights] = useState<Record<string, number>>({});
    /** Место над полем для того, что висит поверх (DeskFloat), и где оно сейчас: от правого верхнего угла поля, точки экрана. */
    const [floatHost, setFloatHost] = useState<HTMLDivElement | null>(null);
    const [floatBox, setFloatBox] = useState<{ right: number; top: number; w: number; h: number } | null>(null);
    useLayoutEffect(() => {
        if (!floatHost) return;
        const measure = () => {
            const outer = box.current;
            const next =
                floatHost.childElementCount > 0 && outer
                    ? {
                          right: outer.clientWidth - floatHost.offsetLeft - floatHost.offsetWidth,
                          top: floatHost.offsetTop,
                          w: floatHost.offsetWidth,
                          h: floatHost.offsetHeight,
                      }
                    : null;
            setFloatBox((old) => (JSON.stringify(old) === JSON.stringify(next) ? old : next));
        };
        measure();
        const sizes = new ResizeObserver(measure);
        sizes.observe(floatHost);
        const parts = new MutationObserver(measure);
        parts.observe(floatHost, { childList: true });
        return () => {
            sizes.disconnect();
            parts.disconnect();
        };
    }, [floatHost]);
    /** Высота виджетов под сценой (витрины), точки стола. */
    const underBox = useRef<HTMLDivElement>(null);
    const [underHeight, setUnderHeight] = useState(0);
    /** Виджет, который тащат или растягивают сейчас: его место — здесь, пока не отпустили. */
    const [live, setLive] = useState<{ key: string; place: WidgetPlace } | null>(null);
    const [map, setMap] = useState<{ items: Array<{ key: string; rect: DOMRect; scene?: boolean }>; view: DOMRect } | null>(null);
    /**
     * На сколько поле опущено под баннером, точек экрана. Мерится по столу как сначала (стартовый сдвиг, сохранённый
     * масштаб): у нетронутого стола сцена — сразу под баннером; у сдвинутого всё поле опускается на то же, пока баннер
     * виден. Сдвиг поля не трогает — ушёл баннер, поле вернулось.
     */
    const [push, setPush] = useState(0);
    const bannerBox = useRef<HTMLDivElement>(null);
    /** Что нарисовано в сцене — его высота (без масштаба поля): под ним встаёт широкий виджет. */
    const sceneBox = useRef<HTMLDivElement>(null);
    const [sceneHeight, setSceneHeight] = useState(SCENE_H - 64);
    useLayoutEffect(() => {
        const element = sceneBox.current;
        if (!element) return;
        const measure = () => setSceneHeight(element.offsetHeight);
        measure();
        const observer = new ResizeObserver(measure);
        observer.observe(element);
        return () => observer.disconnect();
    }, []);
    const view = useRef({ zoom, pan, push });
    view.current = { zoom, pan, push };
    const locked = layout.locked !== false;
    /** Для слушателей, поставленных один раз (колесо, прокрутка). */
    const lockedNow = useRef(locked);
    lockedNow.current = locked;
    /**
     * Ряд замка и масштаба над доком: без мини-карты (окно 1280 точек и уже) он стоит внизу, на линии дока, — кроме
     * окна, где он налез бы на док (у помощника с «+» — уже ~1160 точек). Ряд прижат вправо: его левый край от высоты не
     * зависит, и решение не скачет.
     */
    const tools = useRef<HTMLDivElement>(null);
    const [raised, setRaised] = useState(false);
    useEffect(() => {
        // Док меняется сам по себе: «+» помощника приходит, когда шлюз сказал роль, — за новыми частями дока тоже следим.
        const sizes = new ResizeObserver(() => check());
        const parts = new MutationObserver(() => check());
        let wrap: Element | null = null;
        const check = () => {
            const found = document.querySelector('.dock-wrap');
            if (found && found !== wrap) {
                wrap = found;
                parts.disconnect();
                parts.observe(found, { childList: true });
            }
            const own = [...(wrap?.children ?? [])] as HTMLElement[];
            for (const part of own) sizes.observe(part);
            const row = tools.current?.querySelector('.desk-zoom')?.getBoundingClientRect();
            const right = Math.max(...own.map((part) => part.getBoundingClientRect().right));
            setRaised(!!row && own.length > 0 && right + 12 > row.left);
        };
        check();
        window.addEventListener('resize', check);
        return () => {
            sizes.disconnect();
            parts.disconnect();
            window.removeEventListener('resize', check);
        };
    }, []);

    // Баннер над столом — поле ниже его на BANNER_GAP: сцена с её строкой («Мозг не выбран…») видна целиком.
    useLayoutEffect(() => {
        const node = bannerBox.current;
        const outer = box.current;
        if (!node || !outer) {
            setPush(0);
            return;
        }
        const measure = () => {
            const sceneTop = outer.clientHeight / 2 + startPan.current.y - (SCENE_H / 2) * (layout.zoom ?? startZoom.current);
            setPush(Math.max(0, Math.round(node.offsetTop + node.offsetHeight + BANNER_GAP - sceneTop)));
        };
        measure();
        const sizes = new ResizeObserver(measure);
        sizes.observe(node);
        sizes.observe(outer);
        return () => sizes.disconnect();
    }, [!!banner, layout.zoom, withUnder, underHeight, sceneHeight]);

    // Масштаб и сдвиг — в раскладку, когда человек остановился. Вид как сначала там не нужен: стартовый стол шлюз не хранит.
    // У закреплённого стола сдвиг колесом — только посмотреть: он не хранится.
    useEffect(() => {
        if (locked) return;
        const id = window.setTimeout(() => {
            const saved = layout.pan ?? startPan.current;
            if (zoom !== (layout.zoom ?? startZoom.current) || pan.x !== saved.x || pan.y !== saved.y) actions.view(zoom, pan);
        }, 400);
        return () => window.clearTimeout(id);
    }, [zoom, pan.x, pan.y, locked]);
    // Вид из раскладки: «Расставить заново» вернуло его к началу или стол пришёл со шлюза (его двигали в другом браузере).
    // Виджет под колонкой пришёл позже стола (окошко приложения грузится само), вырос или окно поменялось — начало стола
    // пересчитывается; нетронутый в этот раз стол встаёт в новое начало.
    const half = SCENE_W / 2;
    // Под сценой — во всю её ширину — виджеты, что просят встать там (`under`), пока их не передвинули руками; у
    // остальных — свои места (desk-places.ts).
    const under = widgets.filter((widget) => widget.under && !layout.places[widget.key] && live?.key !== widget.key);
    const rest = widgets.filter((widget) => !under.includes(widget));
    const underWide = under.some((widget) => widget.wide);
    const underWidth = underWide ? UNDER_WIDE : SCENE_W;
    const laid: Laid[] = rest.map((widget) => ({
        key: widget.key,
        place: live?.key === widget.key ? live.place : layout.places[widget.key],
        height: heights[widget.key],
    }));
    /** Видные виджеты со своим местом, точки стола. */
    const shownRects = laid.flatMap((entry) =>
        entry.place && entry.height ? [{ key: entry.key, x: entry.place.x, y: entry.place.y, w: entry.place.w ?? WIDTH, h: entry.height }] : [],
    );
    const underAt = underTop(underWide ? top + sceneHeight - UNDER_WIDE_OVERLAP : SCENE_H / 2 + 24, underWidth / 2, shownRects, COLUMN_GAP / 2);
    useLayoutEffect(() => {
        const settle = () => {
            const outer = box.current;
            if (!outer) return;
            const rect = outer.getBoundingClientRect();
            const dock = document.querySelector('.dock-wrap > *')?.getBoundingClientRect();
            const room = (dock && dock.height > 0 ? Math.min(rect.height, dock.top - rect.top) : rect.height) - 12;
            const node = bannerBox.current;
            const head = Math.max(24, node ? node.offsetTop + node.offsetHeight + BANNER_GAP : 0);
            const need = underHeight > 0 ? underAt + underHeight - top : 0;
            const start = startOf(withUnder, outer.clientHeight, room, head, need);
            startPan.current = start.pan;
            startZoom.current = start.zoom;
            const fresh = !layout.pan && layout.zoom === undefined && !touched.current;
            const zoomTo = layout.zoom ?? (fresh || !layout.pan ? start.zoom : view.current.zoom);
            const panTo = layout.pan ?? (fresh ? start.pan : view.current.pan);
            if (zoomTo !== view.current.zoom) setZoom(zoomTo);
            if (panTo.x !== view.current.pan.x || panTo.y !== view.current.pan.y) setPan(panTo);
        };
        settle();
        window.addEventListener('resize', settle);
        return () => window.removeEventListener('resize', settle);
    }, [layout.zoom, layout.pan?.x, layout.pan?.y, withUnder, underWide, underHeight, underAt, sceneHeight, !!banner]);

    /** Точка экрана → точка стола. */
    const toPlane = (clientX: number, clientY: number): Point => {
        const rect = box.current?.getBoundingClientRect();
        if (!rect) return { x: 0, y: 0 };
        const { zoom: z, pan: p, push: down } = view.current;
        return { x: (clientX - rect.left - rect.width / 2 - p.x) / z, y: (clientY - rect.top - rect.height / 2 - down - p.y) / z };
    };

    // Мерило для посадки черновика: точка поля по месту на экране.
    useEffect(() => {
        const own = (element: Element): WidgetPlace => {
            const rect = element.getBoundingClientRect();
            const corner = toPlane(rect.left, rect.top);
            return { x: Math.round(corner.x), y: Math.round(corner.y), w: Math.round(rect.width / view.current.zoom) };
        };
        measure = own;
        return () => {
            if (measure === own) measure = null;
        };
    }, []);

    const zoomAt = (next: number, clientX?: number, clientY?: number) => {
        const rect = box.current?.getBoundingClientRect();
        if (!rect) return;
        const target = clamp(Math.round(next * 100) / 100, MIN_ZOOM, MAX_ZOOM);
        const cx = clientX ?? rect.left + rect.width / 2;
        const cy = clientY ?? rect.top + rect.height / 2;
        const point = toPlane(cx, cy);
        touched.current = true;
        setZoom(target);
        setPan({ x: cx - rect.left - rect.width / 2 - point.x * target, y: cy - rect.top - rect.height / 2 - view.current.push - point.y * target });
    };

    // Колесо: сдвиг поля; с ⌘/Ctrl (и щипок тачпада) — масштаб у курсора. Над списком, которому есть куда листаться, —
    // сам список. Закреплённый стол колесом сдвигается (посмотреть), но не масштабируется — и ⌘/Ctrl + колесо не
    // масштабирует страницу.
    useEffect(() => {
        const node = box.current;
        if (!node) return;
        const onWheel = (event: WheelEvent) => {
            const target = event.target as HTMLElement;
            if (target.closest('.widget-menu, input, textarea, select')) return;
            const zooming = event.ctrlKey || event.metaKey;
            if (!zooming && scrollsItself(target, node, event.deltaX, event.deltaY)) return;
            event.preventDefault();
            if (zooming) {
                if (!lockedNow.current) zoomAt(view.current.zoom * Math.exp(-event.deltaY * 0.0025), event.clientX, event.clientY);
                return;
            }
            touched.current = true;
            setPan((old) => ({ x: old.x - event.deltaX, y: old.y - event.deltaY }));
        };
        node.addEventListener('wheel', onWheel, { passive: false });
        return () => node.removeEventListener('wheel', onWheel);
    }, []);

    // Поле не прокручивается (overflow: clip, ui/desk.css). Где clip нет, фокус в обрезанном краем виджете прокрутил бы
    // поле мимо pan, и места виджетов разошлись бы с курсором: такой сдвиг уходит в pan (у закреплённого стола — никуда),
    // а прокрутка — обратно в ноль.
    useEffect(() => {
        const node = box.current;
        if (!node) return;
        const onScroll = () => {
            const { scrollLeft: x, scrollTop: y } = node;
            if (!x && !y) return;
            node.scrollLeft = 0;
            node.scrollTop = 0;
            touched.current = true;
            setPan((old) => ({ x: old.x - x, y: old.y - y }));
        };
        node.addEventListener('scroll', onScroll);
        return () => node.removeEventListener('scroll', onScroll);
    }, []);

    // Фон тащат — поле едет.
    const onBackground = (event: PointerEvent) => {
        if (locked || event.button !== 0) return;
        if ((event.target as HTMLElement).closest('.widget, .widget-bare, .desk-scene, .desk-add-tile, .desk-zoom, .desk-map, .desk-float, .desk-slot, button, a, input')) return;
        const from = view.current.pan;
        touched.current = true;
        drag(
            event,
            (dx, dy) => setPan({ x: from.x + dx, y: from.y + dy }),
            () => undefined,
        );
    };

    /** Место виджета на столе сейчас: из раскладки, а не переставленного — по его месту в столбце. */
    const placeOf = (key: string): WidgetPlace | null => {
        if (live?.key === key) return live.place;
        const saved = layout.places[key];
        if (saved) return saved;
        const node = plane.current?.querySelector<HTMLElement>(`[data-widget="${CSS.escape(key)}"]`);
        if (!node) return null;
        const rect = node.getBoundingClientRect();
        const corner = toPlane(rect.left, rect.top);
        return { x: corner.x, y: corner.y, w: rect.width / view.current.zoom };
    };

    const grip = (key: string) => (event: PointerEvent) => {
        if (lockedNow.current || event.button !== 0 || (event.target as HTMLElement).closest('button, a, input')) return;
        const start = placeOf(key);
        if (!start) return;
        event.preventDefault();
        let last = start;
        drag(
            event,
            (dx, dy) => {
                last = { ...start, x: start.x + dx / view.current.zoom, y: start.y + dy / view.current.zoom };
                setLive({ key, place: last });
            },
            (moved) => {
                setLive(null);
                if (moved) actions.place(key, { ...last, x: Math.round(last.x), y: Math.round(last.y) });
            },
        );
    };

    const resize = (key: string) => (event: PointerEvent) => {
        if (lockedNow.current || event.button !== 0) return;
        const start = placeOf(key);
        if (!start) return;
        event.preventDefault();
        event.stopPropagation();
        const width = start.w ?? WIDTH;
        let last = start;
        drag(
            event,
            (dx) => {
                last = { ...start, w: clamp(width + dx / view.current.zoom, MIN_W, MAX_W) };
                setLive({ key, place: last });
            },
            (moved) => {
                setLive(null);
                if (moved) actions.place(key, { ...last, x: Math.round(last.x), y: Math.round(last.y), w: Math.round(last.w ?? WIDTH) });
            },
        );
    };

    /** Шире или уже — из меню виджета (с клавиатуры). */
    const widen = (key: string, step: number) => {
        const start = placeOf(key);
        if (!start || lockedNow.current) return;
        actions.place(key, { ...start, x: Math.round(start.x), y: Math.round(start.y), w: Math.round(clamp((start.w ?? WIDTH) + step, MIN_W, MAX_W)) });
    };

    /** Где «Новый виджет» стоял в прошлый раз: там и стоит, пока свободно (desk-places.ts, `keep`). */
    const addAt = useRef<{ x: number; y: number } | null>(null);
    // Новый виджет — на первое свободное место, в том порядке, что раньше вставали столбцы (макет SpaceGateway): слева в
    // 60 точках от колонки, справа — за ней; дальше — вторые столбцы. Сначала — туда, где его видно в начале стола (в
    // окне и над доком); не видно нигде — в дальние столбцы сверху.
    const clientW = box.current?.clientWidth ?? window.innerWidth;
    const clientH = box.current?.clientHeight ?? window.innerHeight;
    const showcase: Rect | null = under.length > 0 && underHeight > 0 ? { x: -underWidth / 2, y: underAt, w: underWidth, h: underHeight } : null;
    const arranged = arrange(laid, [{ x: -half, y: top, w: SCENE_W + sceneExtra, h: sceneHeight }, ...(showcase ? [showcase] : [])], {
        columns: [-half - GAP - WIDTH, half + sceneExtra + GAP, -half - GAP - 2 * WIDTH - COLUMN_GAP, half + sceneExtra + GAP + WIDTH + COLUMN_GAP],
        width: WIDTH,
        top,
        gap: ROW_GAP,
        sight: {
            left: -clientW / 2 / startZoom.current,
            right: clientW / 2 / startZoom.current,
            // Низ видного — над доком (около 96 точек экрана).
            bottom: (clientH / 2 - push - startPan.current.y - 96) / startZoom.current,
        },
        guess: GUESS_H,
        addHeight: heights[ADD] ?? GUESS_H,
        keep: addAt.current,
        // Карточка Турбо над полем — в точках стола, как стол начинается: под неё новые не встают (под ней их не видно),
        // а уже стоящие там не двигаются — карточку закрывают крестиком.
        cover: floatBox && {
            x: (clientW / 2 - floatBox.right - floatBox.w - 16 - startPan.current.x) / startZoom.current,
            y: (floatBox.top - clientH / 2 - push - startPan.current.y) / startZoom.current,
            w: (floatBox.w + 32) / startZoom.current,
            h: (floatBox.h + 16) / startZoom.current,
        },
    });

    // Новые места — в раскладку: дальше виджет держит своё. Вырос и лёг на соседа (или на витрину) — кто ниже, отходит
    // вниз. Пока виджет тащат, никто не двигается: отпустили — тогда.
    /**
     * Кому стол уже дал место сам: второй раз — только после «Расставить заново». Страховка от спора за место: две
     * раскладки разом (вторая вкладка, окно, подменённое на ходу) не перекладывают виджет друг у друга без конца.
     */
    const gave = useRef(new Set<string>());
    useLayoutEffect(() => {
        // Где стоит «Новый виджет» — запоминаем, когда новых без места нет: место, что плитка показывала до них, и
        // займёт первый из новых (черновик садится туда, где стояла его плитка).
        if (laid.every((entry) => entry.place || entry.height === 0)) addAt.current = arranged.add;
        // Пока виджет тащат, стол не виден или ещё без размера (окно свёрнуто) — ничего не двигаем и мест не даём.
        if (live || document.visibilityState === 'hidden' || clientW < 320 || clientH < 240) return;
        for (const key of gave.current) if (!rest.some((widget) => widget.key === key)) gave.current.delete(key);
        const next: Record<string, WidgetPlace> = {};
        for (const [key, spot] of Object.entries(arranged.fresh)) {
            if (gave.current.has(key)) continue;
            gave.current.add(key);
            next[key] = spot;
        }
        for (const [key, spot] of Object.entries(pushApart(shownRects, showcase ? [showcase] : [], ROW_GAP))) {
            const place = layout.places[key];
            if (place) next[key] = { ...place, ...spot };
        }
        if (Object.keys(next).length > 0) actions.places(next);
    });

    // Мини-карта: что на столе и где окно, — после каждой отрисовки, по местам в DOM.
    useLayoutEffect(() => {
        const node = plane.current;
        const outer = box.current;
        if (!node || !outer) return;
        // Невидимые (пустой виджет с hideWhileEmpty) — не на карте: их место пусто, а нулевая рамка растянула бы карту.
        const items = [...node.querySelectorAll<HTMLElement>('[data-widget], .desk-scene, .desk-add-tile')]
            .map((element) => ({
                key: element.dataset.widget ?? (element.classList.contains('desk-scene') ? 'scene' : 'add'),
                rect: element.getBoundingClientRect(),
                scene: element.classList.contains('desk-scene'),
            }))
            .filter((item) => item.rect.width > 0 || item.rect.height > 0);
        const next = { items, view: outer.getBoundingClientRect() };
        setMap((old) =>
            old && old.items.length === next.items.length && old.items.every((item, index) => sameRect(item.rect, next.items[index].rect)) && sameRect(old.view, next.view)
                ? old
                : next,
        );
    });

    // Высоты виджетов и места «Новый виджет» — для свободных мест; высота витрины — для начала стола. Виджет меняет
    // высоту сам (пришли данные, пустой стал видимым) — следим за каждым. Спрятанный, пока пуст, — 0.
    useLayoutEffect(() => {
        const node = plane.current;
        if (!node) return;
        const measure = () => {
            const measured: Record<string, number> = {};
            const spot = node.querySelector<HTMLElement>(':scope > .desk-add-spot');
            if (spot) measured[ADD] = spot.offsetHeight;
            for (const slot of node.querySelectorAll<HTMLElement>(':scope > .desk-slot[data-widget]')) measured[slot.dataset.widget ?? ''] = slot.offsetHeight;
            setHeights((old) =>
                Object.keys(measured).length === Object.keys(old).length && Object.entries(measured).every(([key, value]) => Math.abs((old[key] ?? -9) - value) < 2)
                    ? old
                    : measured,
            );
            const below = underBox.current?.offsetHeight ?? 0;
            setUnderHeight((old) => (Math.abs(below - old) > 1 ? below : old));
        };
        measure();
        const sizes = new ResizeObserver(measure);
        for (const slot of node.querySelectorAll(':scope > .desk-slot, :scope > .desk-slot > *, :scope > .desk-add-spot > *, .desk-under')) sizes.observe(slot);
        return () => sizes.disconnect();
    });

    const widgetMenu = (widget: DeskWidget): WidgetMenu => ({
        hide: () => actions.hide(widget.key),
        wider: () => widen(widget.key, 60),
        narrower: () => widen(widget.key, -60),
        locked,
    });
    const gripOf = (key: string) => (locked ? undefined : grip(key));

    return (
        <div ref={box} class={`desk-canvas${locked ? ' is-locked' : ''}`} onPointerDown={(event) => onBackground(event as PointerEvent)}>
            {banner && (
                <div ref={bannerBox} class="desk-canvas-banner">
                    {banner}
                </div>
            )}
            {/* Что висит поверх стола у правого края (карточка Турбо): сцена кладёт его сюда сама (useDeskFloat). */}
            <div ref={setFloatHost} class="desk-float" />
            {/* Опускание под баннером — своим слоем с плавным ходом: сдвиг и масштаб поля идут без задержки. */}
            <DeskFloat.Provider value={floatHost}>
            <div class="desk-push" style={push ? { transform: `translateY(${push}px)` } : undefined}>
                <div ref={plane} class="desk-plane" style={{ transform: `translate(${pan.x}px, ${pan.y}px) scale(${zoom})` }}>
                    <div ref={sceneBox} class="desk-scene-spot" style={{ left: `${-half}px`, top: `${top}px`, width: `${SCENE_W + sceneExtra}px` }}>
                        {scene}
                    </div>
                    {under.length > 0 && (
                        <div ref={underBox} class="desk-under" style={{ left: `${-underWidth / 2}px`, top: `${underAt}px`, width: `${underWidth}px` }}>
                            {under.map((widget) => (
                                <div
                                    key={widget.key}
                                    data-widget={widget.key}
                                    class="desk-slot"
                                    style={widget.wide || underWidth === SCENE_W ? undefined : { width: `${SCENE_W}px`, marginInline: 'auto' }}
                                >
                                    {render(widget, widgetMenu(widget), gripOf(widget.key))}
                                </div>
                            ))}
                        </div>
                    )}
                    {rest.map((widget) => {
                        const spot = arranged.spots[widget.key];
                        return (
                            <div
                                key={widget.key}
                                data-widget={widget.key}
                                class={`desk-slot desk-placed${live?.key === widget.key ? ' is-moving' : ''}`}
                                style={{ left: `${spot?.x ?? -half}px`, top: `${spot?.y ?? top}px`, width: `${spot?.w ?? WIDTH}px` }}
                            >
                                {render(widget, widgetMenu(widget), gripOf(widget.key))}
                                {!locked && <span class="desk-resize" aria-hidden="true" onPointerDown={(event) => resize(widget.key)(event as PointerEvent)} />}
                            </div>
                        );
                    })}
                    {/* «Новый виджет» — там, где встанет следующий (docs/plans/voice-creation.md: вещь рождается там, где будет стоять). */}
                    <div class="desk-add-spot" style={{ left: `${arranged.add.x}px`, top: `${arranged.add.y}px`, width: `${WIDTH}px` }}>
                        {addTile}
                    </div>
                </div>
            </div>
            </DeskFloat.Provider>

            {/* Закреплён — в углу только замок: остальное ряда не видно, но место держит, и замок не прыгает, когда стол
                открывают; карта над рядом появляется вместе с ними. */}
            <div ref={tools} class={`desk-tools${raised ? ' is-raised' : ''}${locked ? ' is-locked' : ''}`}>
                {map && !locked && <MiniMap map={map} onGo={(dx, dy) => setPan((old) => ({ x: old.x - dx, y: old.y - dy }))} />}
                <div class="desk-zoom">
                    <button
                        class="desk-zoom-btn desk-tool"
                        aria-label={tr.desk.rearrange}
                        title={tr.desk.rearrange}
                        disabled={locked}
                        onClick={() => {
                            gave.current.clear();
                            actions.reset();
                        }}
                    >
                        <IconGrid size={14} />
                    </button>
                    <button class="desk-zoom-btn desk-tool" aria-label={tr.desk.smaller} disabled={locked} onClick={() => zoomAt(zoom - 0.1)}>
                        <IconMinus size={14} />
                    </button>
                    <button class="desk-zoom-level desk-tool" aria-label={tr.desk.zoomReset} disabled={locked} onClick={() => zoomAt(1)}>
                        {Math.round(zoom * 100)} %
                    </button>
                    <button class="desk-zoom-btn desk-tool" aria-label={tr.desk.bigger} disabled={locked} onClick={() => zoomAt(zoom + 0.1)}>
                        <IconPlus size={14} />
                    </button>
                    <button
                        class="desk-zoom-btn desk-lock"
                        aria-label={tr.desk.lockDesk}
                        aria-pressed={locked}
                        title={locked ? tr.desk.unlockDesk : tr.desk.lockDesk}
                        onClick={() => actions.lock(!locked)}
                    >
                        {locked ? <IconLock size={14} /> : <IconUnlock size={14} />}
                    </button>
                </div>
            </div>
        </div>
    );
}

/** Ключ места «Новый виджет» в высотах столбцов. */
const ADD = ' add';

const sameRect = (a: DOMRect, b: DOMRect): boolean =>
    Math.abs(a.left - b.left) < 0.5 && Math.abs(a.top - b.top) < 0.5 && Math.abs(a.width - b.width) < 0.5 && Math.abs(a.height - b.height) < 0.5;

/**
 * Мини-карта стола: виджеты и колонка — плашками, окно — рамкой. Нажатие ведёт окно туда: `onGo` — на сколько
 * сдвинуть его в точках экрана. Она есть только у открытого стола.
 */
function MiniMap({ map, onGo }: { map: { items: Array<{ key: string; rect: DOMRect; scene?: boolean }>; view: DOMRect }; onGo: (dx: number, dy: number) => void }) {
    const W = 132;
    const H = 84;
    const all = [...map.items.map((item) => item.rect), map.view];
    const left = Math.min(...all.map((rect) => rect.left));
    const top = Math.min(...all.map((rect) => rect.top));
    const right = Math.max(...all.map((rect) => rect.right));
    const bottom = Math.max(...all.map((rect) => rect.bottom));
    const scale = Math.min((W - 8) / Math.max(1, right - left), (H - 8) / Math.max(1, bottom - top));
    const ox = (W - (right - left) * scale) / 2;
    const oy = (H - (bottom - top) * scale) / 2;
    const box = (rect: DOMRect) => ({
        left: `${ox + (rect.left - left) * scale}px`,
        top: `${oy + (rect.top - top) * scale}px`,
        width: `${Math.max(2, rect.width * scale)}px`,
        height: `${Math.max(2, rect.height * scale)}px`,
    });
    const hidden = map.items.some(
        (item) => item.rect.right < map.view.left || item.rect.left > map.view.right || item.rect.bottom < map.view.top || item.rect.top > map.view.bottom,
    );
    return (
        <button
            class="desk-map"
            aria-label={hidden ? tr.desk.mapHidden : tr.desk.map}
            onClick={(event) => {
                const own = (event.currentTarget as HTMLElement).getBoundingClientRect();
                // Куда нажали — в точки экрана; окно встаёт серединой туда.
                const x = left + (event.clientX - own.left - ox) / scale;
                const y = top + (event.clientY - own.top - oy) / scale;
                onGo(x - (map.view.left + map.view.width / 2), y - (map.view.top + map.view.height / 2));
            }}
        >
            {map.items.map((item) => (
                <span key={item.key} class={`desk-map-item${item.scene ? ' is-scene' : ''}${item.key === 'add' ? ' is-add' : ''}`} style={box(item.rect)} />
            ))}
            <span class="desk-map-view" style={box(map.view)} />
        </button>
    );
}
