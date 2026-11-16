// Свободный стол (docs/plans/desk.md, этап 3; макеты SpaceGateway, SpaceHelper): поле без краёв, колонка — в начале
// координат, виджеты — где их поставили. Виджет перетаскивают за шапку и растягивают за угол; поле сдвигают
// перетаскиванием фона и колесом, масштаб — кнопками, ⌘/Ctrl + колесом (щипок на тачпаде); мини-карта показывает,
// что есть за краем, и ведёт туда. Ещё не переставленные виджеты стоят столбцами по сторонам от колонки. Колесо над
// списком в виджете, который ещё есть куда листать, листает его, а не поле.
// Замок в углу закрепляет стол: поле не едет и не масштабируется, виджеты не двигаются и не тянутся, пункты раскладки в
// их меню не нажимаются. Стол начинается закреплённым, и тогда в углу только замок; открыли — рядом появляются
// «Расставить заново», масштаб и мини-карта. Места, масштаб, сдвиг и замок — в раскладке стола: её хранит шлюз, свою у
// каждой роли (shell/widgets.ts). Плашки над столом (баннер: «Готовлю слух», флешка…) опускают поле, пока видны: сцена
// стоит под ними, а не за ними.

import type { ComponentChildren } from 'preact';
import { useEffect, useLayoutEffect, useRef, useState } from 'preact/hooks';

import { IconGrid, IconLock, IconMinus, IconPlus, IconUnlock } from '../icons';
import { DeskActions, DeskLayout, DeskWidget, WidgetPlace } from '../shell/widgets';
import type { WidgetMenu } from '../ui/widget';
import { tr } from '../i18n';

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
/** Между баннером и сценой под ним, точек экрана. */
const BANNER_GAP = 16;

interface Point {
    x: number;
    y: number;
}

/** Сдвиг поля, пока его не трогали: колонка чуть выше середины окна. */
const START: Point = { x: 0, y: -40 };

const clamp = (value: number, min: number, max: number) => Math.min(max, Math.max(min, value));

/**
 * Сдвиг поля, пока его не трогали, когда под колонкой стоит виджет (`under`, «Лента»): колонка поднимается, насколько
 * позволяет место над ней (до 24 точек от верха поля, не выше 240), — чтобы виджет под ней было видно сразу, а не одну
 * его шапку. `height` — высота поля.
 */
function startOf(withUnder: boolean, height: number): Point {
    if (!withUnder) return START;
    return { x: 0, y: Math.round(clamp(SCENE_H / 2 + 24 - height / 2, -240, START.y)) };
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
    banner: ComponentChildren;
    /** `grip` — перетаскивание за шапку; у закреплённого стола его нет. */
    render: (widget: DeskWidget, menu: WidgetMenu, grip?: (event: PointerEvent) => void) => ComponentChildren;
}

export function DeskCanvas({ widgets, layout, actions, scene, addTile, banner, render }: Props) {
    const box = useRef<HTMLDivElement>(null);
    const plane = useRef<HTMLDivElement>(null);
    const [zoom, setZoom] = useState(layout.zoom ?? 1);
    const withUnder = widgets.some((widget) => widget.under && !layout.places[widget.key]);
    /** С чего стол начинается, пока его не двигали: от него же «вид как сначала» не сохраняется. */
    const startPan = useRef<Point>(startOf(withUnder, window.innerHeight - 64));
    const [pan, setPan] = useState<Point>(layout.pan ?? startPan.current);
    /** Виджет, который тащат или растягивают сейчас: его место — здесь, пока не отпустили. */
    const [live, setLive] = useState<{ key: string; place: WidgetPlace } | null>(null);
    const [map, setMap] = useState<{ items: Array<{ key: string; rect: DOMRect; scene?: boolean }>; view: DOMRect } | null>(null);
    /** «Новый виджет» под севшими на его место виджетами (docs/plans/voice-creation.md, «Посадка»): на сколько ниже, точки поля. */
    const [addOffset, setAddOffset] = useState(0);
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
            const sceneTop = outer.clientHeight / 2 + startPan.current.y - (SCENE_H / 2) * (layout.zoom ?? 1);
            setPush(Math.max(0, Math.round(node.offsetTop + node.offsetHeight + BANNER_GAP - sceneTop)));
        };
        measure();
        const sizes = new ResizeObserver(measure);
        sizes.observe(node);
        sizes.observe(outer);
        return () => sizes.disconnect();
    }, [!!banner, layout.zoom, withUnder]);

    // Масштаб и сдвиг — в раскладку, когда человек остановился. Вид как сначала там не нужен: стартовый стол шлюз не хранит.
    useEffect(() => {
        const id = window.setTimeout(() => {
            const saved = layout.pan ?? startPan.current;
            if (zoom !== (layout.zoom ?? 1) || pan.x !== saved.x || pan.y !== saved.y) actions.view(zoom, pan);
        }, 400);
        return () => window.clearTimeout(id);
    }, [zoom, pan.x, pan.y]);
    // Вид из раскладки: «Расставить заново» вернуло его к началу или стол пришёл со шлюза (его двигали в другом браузере).
    // Виджет под колонкой пришёл позже стола (окошко приложения грузится само) — начало стола пересчитывается.
    useEffect(() => {
        const zoomTo = layout.zoom ?? 1;
        if (!layout.pan) startPan.current = startOf(withUnder, box.current?.clientHeight ?? window.innerHeight - 64);
        const panTo = layout.pan ?? startPan.current;
        if (zoomTo !== view.current.zoom) setZoom(zoomTo);
        if (panTo.x !== view.current.pan.x || panTo.y !== view.current.pan.y) setPan(panTo);
    }, [layout.zoom, layout.pan?.x, layout.pan?.y, withUnder]);

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
        setZoom(target);
        setPan({ x: cx - rect.left - rect.width / 2 - point.x * target, y: cy - rect.top - rect.height / 2 - view.current.push - point.y * target });
    };

    // Колесо: сдвиг поля; с ⌘/Ctrl (и щипок тачпада) — масштаб у курсора. Над списком, которому есть куда листаться, —
    // сам список. Закреплённый стол стоит, а ⌘/Ctrl + колесо не масштабирует и страницу.
    useEffect(() => {
        const node = box.current;
        if (!node) return;
        const onWheel = (event: WheelEvent) => {
            const target = event.target as HTMLElement;
            if (target.closest('.widget-menu, input, textarea, select')) return;
            const zooming = event.ctrlKey || event.metaKey;
            if (!zooming && scrollsItself(target, node, event.deltaX, event.deltaY)) return;
            event.preventDefault();
            if (lockedNow.current) return;
            if (zooming) zoomAt(view.current.zoom * Math.exp(-event.deltaY * 0.0025), event.clientX, event.clientY);
            else setPan((old) => ({ x: old.x - event.deltaX, y: old.y - event.deltaY }));
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
            if (!lockedNow.current) setPan((old) => ({ x: old.x - x, y: old.y - y }));
        };
        node.addEventListener('scroll', onScroll);
        return () => node.removeEventListener('scroll', onScroll);
    }, []);

    // Фон тащат — поле едет.
    const onBackground = (event: PointerEvent) => {
        if (locked || event.button !== 0) return;
        if ((event.target as HTMLElement).closest('.widget, .widget-bare, .desk-scene, .desk-add-tile, .desk-zoom, .desk-map, .desk-columns > *, button, a, input')) return;
        const from = view.current.pan;
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

    const placed = widgets.filter((widget) => layout.places[widget.key] || live?.key === widget.key);
    // Под сценой — во всю её ширину — виджеты, что просят встать там (`under`), пока их не передвинули руками.
    const under = widgets.filter((widget) => widget.under && !layout.places[widget.key] && live?.key !== widget.key);
    const flowing = widgets.filter((widget) => !widget.under && !layout.places[widget.key] && live?.key !== widget.key);
    // Столбцы по сторонам от колонки: по очереди слева и справа, при многих — второй справа.
    const columns: DeskWidget[][] = [[], [], []];
    flowing.forEach((widget, index) => {
        const many = flowing.length > 6;
        columns[many ? [0, 1, 2][index % 3] : index % 2].push(widget);
    });
    // Как в макете (SpaceGateway): слева столбец в 60 точках от колонки, справа два — второй подрезан краем окна,
    // чтобы было видно, что там ещё есть.
    const half = SCENE_W / 2;
    const underWidth = under.some((widget) => widget.wide) ? UNDER_WIDE : SCENE_W;
    const columnX = [-half - GAP - WIDTH, half + GAP, half + GAP + WIDTH + 40];
    const top = -SCENE_H / 2;

    // Мини-карта: что на столе и где окно, — после каждой отрисовки, по местам в DOM. И «Новый виджет» — ниже тех
    // поставленных виджетов, что встали на его место (черновик сел там, где стояла его плитка).
    useLayoutEffect(() => {
        const node = plane.current;
        const outer = box.current;
        if (!node || !outer) return;
        const spot = node.querySelector<HTMLElement>('.desk-add-spot');
        if (spot) {
            const zoom = view.current.zoom;
            const rect = spot.getBoundingClientRect();
            const top = rect.top - addOffset * zoom;
            let need = 0;
            for (const placed of node.querySelectorAll<HTMLElement>('.desk-placed')) {
                const other = placed.getBoundingClientRect();
                const across = other.left < rect.right && other.right > rect.left;
                if (across && other.bottom > top && other.top < top + rect.height + addOffset * zoom) need = Math.max(need, (other.bottom - top) / zoom + 16);
            }
            if (Math.abs(need - addOffset) > 1) setAddOffset(Math.round(need));
        }
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
            {/* Опускание под баннером — своим слоем с плавным ходом: сдвиг и масштаб поля идут без задержки. */}
            <div class="desk-push" style={push ? { transform: `translateY(${push}px)` } : undefined}>
                <div ref={plane} class="desk-plane" style={{ transform: `translate(${pan.x}px, ${pan.y}px) scale(${zoom})` }}>
                    <div ref={sceneBox} class="desk-scene-spot" style={{ left: `${-half}px`, top: `${top}px`, width: `${SCENE_W}px` }}>
                        {scene}
                    </div>
                    {under.length > 0 && (
                        <div class="desk-under" style={{ left: `${-underWidth / 2}px`, top: `${underWidth === SCENE_W ? SCENE_H / 2 + 24 : top + sceneHeight - UNDER_WIDE_OVERLAP}px`, width: `${underWidth}px` }}>
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
                    {columns.map(
                        (list, index) =>
                            (list.length > 0 || index === 0) && (
                                <div key={index} class="desk-columns" style={{ left: `${columnX[index]}px`, top: `${top}px`, width: `${WIDTH}px` }}>
                                    {list.map((widget) => (
                                        <div key={widget.key} data-widget={widget.key} class="desk-slot">
                                            {render(widget, widgetMenu(widget), gripOf(widget.key))}
                                        </div>
                                    ))}
                                    {index === 0 && (
                                        <div class="desk-add-spot" style={addOffset ? { marginTop: `${addOffset}px` } : undefined}>
                                            {addTile}
                                        </div>
                                    )}
                                </div>
                            ),
                    )}
                    {placed.map((widget) => {
                        const place = live?.key === widget.key ? live.place : layout.places[widget.key];
                        return (
                            <div
                                key={widget.key}
                                data-widget={widget.key}
                                class={`desk-slot desk-placed${live?.key === widget.key ? ' is-moving' : ''}`}
                                style={{ left: `${place.x}px`, top: `${place.y}px`, width: `${place.w ?? WIDTH}px` }}
                            >
                                {render(widget, widgetMenu(widget), gripOf(widget.key))}
                                {!locked && <span class="desk-resize" aria-hidden="true" onPointerDown={(event) => resize(widget.key)(event as PointerEvent)} />}
                            </div>
                        );
                    })}
                </div>
            </div>

            {/* Закреплён — в углу только замок: остальное ряда не видно, но место держит, и замок не прыгает, когда стол
                открывают; карта над рядом появляется вместе с ними. */}
            <div ref={tools} class={`desk-tools${raised ? ' is-raised' : ''}${locked ? ' is-locked' : ''}`}>
                {map && !locked && <MiniMap map={map} onGo={(dx, dy) => setPan((old) => ({ x: old.x - dx, y: old.y - dy }))} />}
                <div class="desk-zoom">
                    <button class="desk-zoom-btn desk-tool" aria-label={tr.desk.rearrange} title={tr.desk.rearrange} disabled={locked} onClick={() => actions.reset()}>
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
