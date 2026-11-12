// Свободный стол (docs/plans/desk.md, этап 3; макеты SpaceGateway, SpaceHelper): поле без краёв, колонка — в начале
// координат, виджеты — где их поставили. Виджет перетаскивают за шапку и растягивают за угол; поле сдвигают
// перетаскиванием фона и колесом, масштаб — кнопками, ⌘/Ctrl + колесом (щипок на тачпаде); мини-карта показывает,
// что есть за краем, и ведёт туда. Ещё не переставленные виджеты стоят столбцами по сторонам от колонки. Колесо над
// списком в виджете, который ещё есть куда листать, листает его, а не поле.
// Замок рядом с масштабом закрепляет стол: поле не едет и не масштабируется, виджеты не двигаются и не тянутся, пункты
// раскладки в их меню не нажимаются. Места, масштаб, сдвиг и замок — в раскладке стола: её хранит шлюз, свою у каждой
// роли (shell/widgets.ts).

import type { ComponentChildren } from 'preact';
import { useEffect, useLayoutEffect, useRef, useState } from 'preact/hooks';

import { IconGrid, IconLock, IconMinus, IconPlus, IconUnlock } from '../icons';
import { DeskActions, DeskLayout, DeskWidget, WidgetPlace } from '../shell/widgets';
import type { WidgetMenu } from '../ui/widget';

const MIN_ZOOM = 0.5;
const MAX_ZOOM = 1.6;
/** Ширина виджета: столбец — 300, растянуть можно от 240 до 640. */
const WIDTH = 300;
const MIN_W = 240;
const MAX_W = 640;
/** Сцена колонки: ширина и высота на столе. */
export const SCENE_W = 560;
export const SCENE_H = 520;
const GAP = 60;

interface Point {
    x: number;
    y: number;
}

/** Сдвиг поля, пока его не трогали: колонка чуть выше середины окна. */
const START: Point = { x: 0, y: -40 };

const clamp = (value: number, min: number, max: number) => Math.min(max, Math.max(min, value));

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
    const [pan, setPan] = useState<Point>(layout.pan ?? START);
    /** Виджет, который тащат или растягивают сейчас: его место — здесь, пока не отпустили. */
    const [live, setLive] = useState<{ key: string; place: WidgetPlace } | null>(null);
    const [map, setMap] = useState<{ items: Array<{ key: string; rect: DOMRect; scene?: boolean }>; view: DOMRect } | null>(null);
    /** «Новый виджет» под севшими на его место виджетами (docs/plans/voice-creation.md, «Посадка»): на сколько ниже, точки поля. */
    const [addOffset, setAddOffset] = useState(0);
    const view = useRef({ zoom, pan });
    view.current = { zoom, pan };
    const locked = layout.locked === true;
    /** Для слушателей, поставленных один раз (колесо, прокрутка). */
    const lockedNow = useRef(locked);
    lockedNow.current = locked;

    // Масштаб и сдвиг — в раскладку, когда человек остановился. Вид как сначала там не нужен: стартовый стол шлюз не хранит.
    useEffect(() => {
        const id = window.setTimeout(() => {
            const saved = layout.pan ?? START;
            if (zoom !== (layout.zoom ?? 1) || pan.x !== saved.x || pan.y !== saved.y) actions.view(zoom, pan);
        }, 400);
        return () => window.clearTimeout(id);
    }, [zoom, pan.x, pan.y]);
    // Вид из раскладки: «Расставить заново» вернуло его к началу или стол пришёл со шлюза (его двигали в другом браузере).
    useEffect(() => {
        const zoomTo = layout.zoom ?? 1;
        const panTo = layout.pan ?? START;
        if (zoomTo !== view.current.zoom) setZoom(zoomTo);
        if (panTo.x !== view.current.pan.x || panTo.y !== view.current.pan.y) setPan(panTo);
    }, [layout.zoom, layout.pan?.x, layout.pan?.y]);

    /** Точка экрана → точка стола. */
    const toPlane = (clientX: number, clientY: number): Point => {
        const rect = box.current?.getBoundingClientRect();
        if (!rect) return { x: 0, y: 0 };
        const { zoom: z, pan: p } = view.current;
        return { x: (clientX - rect.left - rect.width / 2 - p.x) / z, y: (clientY - rect.top - rect.height / 2 - p.y) / z };
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
        setPan({ x: cx - rect.left - rect.width / 2 - point.x * target, y: cy - rect.top - rect.height / 2 - point.y * target });
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
    const flowing = widgets.filter((widget) => !layout.places[widget.key] && live?.key !== widget.key);
    // Столбцы по сторонам от колонки: по очереди слева и справа, при многих — второй справа.
    const columns: DeskWidget[][] = [[], [], []];
    flowing.forEach((widget, index) => {
        const many = flowing.length > 6;
        columns[many ? [0, 1, 2][index % 3] : index % 2].push(widget);
    });
    // Как в макете (SpaceGateway): слева столбец в 60 точках от колонки, справа два — второй подрезан краем окна,
    // чтобы было видно, что там ещё есть.
    const half = SCENE_W / 2;
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
        const items = [...node.querySelectorAll<HTMLElement>('[data-widget], .desk-scene, .desk-add-tile')].map((element) => ({
            key: element.dataset.widget ?? (element.classList.contains('desk-scene') ? 'scene' : 'add'),
            rect: element.getBoundingClientRect(),
            scene: element.classList.contains('desk-scene'),
        }));
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
            {banner && <div class="desk-canvas-banner">{banner}</div>}
            <div ref={plane} class="desk-plane" style={{ transform: `translate(${pan.x}px, ${pan.y}px) scale(${zoom})` }}>
                <div class="desk-scene-spot" style={{ left: `${-half}px`, top: `${top}px`, width: `${SCENE_W}px` }}>
                    {scene}
                </div>
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

            <div class="desk-tools">
                <div class="desk-zoom">
                    <button
                        class="desk-zoom-btn"
                        aria-label="Закрепить стол"
                        aria-pressed={locked}
                        title={locked ? 'Открепить стол' : 'Закрепить стол'}
                        onClick={() => actions.lock(!locked)}
                    >
                        {locked ? <IconLock size={14} /> : <IconUnlock size={14} />}
                    </button>
                    <button class="desk-zoom-btn" aria-label="Расставить заново" title="Расставить заново" disabled={locked} onClick={() => actions.reset()}>
                        <IconGrid size={14} />
                    </button>
                    <button class="desk-zoom-btn" aria-label="Мельче" disabled={locked} onClick={() => zoomAt(zoom - 0.1)}>
                        <IconMinus size={14} />
                    </button>
                    <button class="desk-zoom-level" aria-label="Масштаб 100 %" disabled={locked} onClick={() => zoomAt(1)}>
                        {Math.round(zoom * 100)} %
                    </button>
                    <button class="desk-zoom-btn" aria-label="Крупнее" disabled={locked} onClick={() => zoomAt(zoom + 0.1)}>
                        <IconPlus size={14} />
                    </button>
                </div>
                {map && <MiniMap map={map} locked={locked} onGo={(dx, dy) => setPan((old) => ({ x: old.x - dx, y: old.y - dy }))} />}
            </div>
        </div>
    );
}

const sameRect = (a: DOMRect, b: DOMRect): boolean =>
    Math.abs(a.left - b.left) < 0.5 && Math.abs(a.top - b.top) < 0.5 && Math.abs(a.width - b.width) < 0.5 && Math.abs(a.height - b.height) < 0.5;

/**
 * Мини-карта стола: виджеты и колонка — плашками, окно — рамкой. Нажатие ведёт окно туда: `onGo` — на сколько
 * сдвинуть его в точках экрана. У закреплённого стола карта только показывает.
 */
function MiniMap({
    map,
    locked,
    onGo,
}: {
    map: { items: Array<{ key: string; rect: DOMRect; scene?: boolean }>; view: DOMRect };
    locked: boolean;
    onGo: (dx: number, dy: number) => void;
}) {
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
            disabled={locked}
            aria-label={locked ? 'Карта стола: стол закреплён' : hidden ? 'Карта стола: виджеты есть и за краем. Нажмите, куда перейти' : 'Карта стола'}
            onClick={(event) => {
                if (locked) return;
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
