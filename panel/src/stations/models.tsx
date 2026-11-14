import { useId } from 'preact/hooks';
import type { FunctionComponent, VNode } from 'preact';

import { tr } from '../i18n';
import { useWallTime } from './clock';
import { colorInfo, type ColorInfo } from './colors';
import { Defs, ellipse, Frame, FrontDisplay, Light, ringSpot, round, roundedRect, Shadow, skinOf, timeText, useReducedMotion, stageWord, type DisplayBox, type LightSpot, type Skin } from './kit';
import './stations.css';
import type { StationArtProps, StationGlyphProps, StationStage } from './types';

// Вся линейка Станций по фотографиям Яндекса: корпус каждой модели нарисован отдельно, свет Алисы, дисплей и
// анимация общие (kit.tsx). Рисунок должен читаться и в карточке на 80 пикселей: поэтому у каждой модели — свой
// силуэт (шайба, мягкий цилиндр, кирпич, куб, туба, планшет) и одна-две приметы (кольцо, ободок, ушко, часы).

/** Что модель знает о рисунке, который сейчас собирает. */
interface Ctx {
    s: Skin;
    id: (name: string) => string;
    url: (name: string) => string;
    clock: boolean;
    sticker: boolean;
    ring: string;
    stage: StationStage;
    offline?: boolean;
}

/** Что модель отдаёт рисунку: корпус, где у неё свет и дисплей, ширину тени. */
interface Drawn {
    /** Под корпусом: подсветка стены, тень планшета. */
    under?: VNode;
    body: VNode;
    light: LightSpot;
    display?: DisplayBox;
    /** Светодиоды дисплея не корпусного цвета (экран планшета, BORK). */
    displaySkin?: Skin;
    over?: VNode;
    shadow: number;
}

interface Model {
    title: string;
    /** Цвет корпуса, если расцветка незнакома модели. */
    fallback: string;
    /** Может ли у модели быть наклейка: у Лайт 2 в коробке лежат наклейки-котики. */
    sticker?: boolean;
    draw: (ctx: Ctx) => Drawn;
    glyph: (c: ColorInfo) => VNode;
}

// ——— Детали корпусов ————————————————————————————————————————————————————————————————————————————————————

/** Цилиндр: боковина от центра крышки до низа. */
const cylinder = (cx: number, top: number, bottom: number, rx: number, ry: number): string =>
    `M${round(cx - rx)} ${top}L${round(cx - rx)} ${bottom}A${rx} ${ry} 0 0 0 ${round(cx + rx)} ${bottom}L${round(cx + rx)} ${top}A${rx} ${ry} 0 0 0 ${round(cx - rx)} ${top}Z`;

/** Цилиндр с тканью или гладким пластиком, с объёмом: блик слева, тень снизу. */
function Drum({ ctx, top, bottom, rx, ry, fabric, fill }: { ctx: Ctx; top: number; bottom: number; rx: number; ry: number; fabric?: boolean; fill?: string }) {
    const d = cylinder(120, top, bottom, rx, ry);
    return (
        <>
            <path d={d} fill={fill ?? ctx.url(fabric ? 'cloth' : 'body')} stroke={ctx.s.edge} stroke-width="0.8" />
            {fabric && <path d={d} fill={ctx.url('fabric')} />}
            <path d={d} fill={ctx.url('vshade')} />
            <ellipse cx={120 - rx * 0.55} cy={(top + bottom) / 2} rx={rx * 0.22} ry={(bottom - top) * 0.42} fill={ctx.url('spec')} opacity={fabric ? 0.22 : 0.42} />
        </>
    );
}

/** Крышка цилиндра: эллипс, у мягких моделей — со скруглённым краем (светлый ободок вокруг плоской части). */
function Lid({ ctx, cy, rx, ry, soft = 0, fill, fabric }: { ctx: Ctx; cy: number; rx: number; ry: number; soft?: number; fill?: string; fabric?: boolean }) {
    const irx = rx - soft,
        iry = ry - (soft * ry) / rx;
    return (
        <>
            <ellipse cx="120" cy={cy} rx={rx} ry={ry} fill={soft ? ctx.url('body') : (fill ?? ctx.url('top'))} />
            {soft > 0 && <ellipse cx="120" cy={cy - 1} rx={irx} ry={iry} fill={fill ?? ctx.url('top')} />}
            {fabric && <ellipse cx="120" cy={cy} rx={rx} ry={ry} fill={ctx.url('fabric')} opacity="0.7" />}
            <ellipse cx={120 - rx * 0.28} cy={cy - ry * 0.3} rx={rx * 0.45} ry={ry * 0.42} fill={ctx.url('spec')} opacity="0.4" />
        </>
    );
}

/** Коробка в лёгкой перспективе: фасад, крышка уходит вправо-вверх, справа видна боковина. */
interface BoxGeo {
    x0: number;
    x1: number;
    yt: number;
    yb: number;
    dx: number;
    dy: number;
    r: number;
}

function boxGeo(w: number, h: number, dx: number, dy: number, r: number, ground = 170): BoxGeo {
    const x0 = round(120 - (w + dx) / 2);
    return { x0, x1: x0 + w, yt: ground - h, yb: ground, dx, dy, r };
}

const topFace = (g: BoxGeo, k = 1): string => {
    const cx = (g.x0 + g.x1 + g.dx) / 2,
        cy = g.yt + g.dy / 2;
    const pts = [
        [g.x0, g.yt],
        [g.x0 + g.dx, g.yt + g.dy],
        [g.x1 + g.dx, g.yt + g.dy],
        [g.x1, g.yt],
    ].map(([x, y]) => [cx + (x - cx) * k, cy + (y - cy) * k]);
    return `M${pts.map(([x, y]) => `${round(x)} ${round(y)}`).join('L')}Z`;
};

/** Боковина заходит под фасад на радиус скругления: иначе у скруглённого угла видна щель. */
const sideFace = (g: BoxGeo): string =>
    `M${g.x1 - g.r} ${g.yt}L${g.x1} ${g.yt}L${g.x1 + g.dx} ${g.yt + g.dy}L${g.x1 + g.dx} ${g.yb + g.dy - g.r * 0.6}L${g.x1} ${g.yb}L${g.x1 - g.r} ${g.yb}Z`;

const topCenter = (g: BoxGeo): { x: number; y: number } => ({ x: (g.x0 + g.x1 + g.dx) / 2, y: g.yt + g.dy / 2 });

function Box({ ctx, g, fabric = true, top, front }: { ctx: Ctx; g: BoxGeo; fabric?: boolean; top?: string; front?: string }) {
    const frontD = roundedRect(g.x0, g.yt, g.x1 - g.x0, g.yb - g.yt, g.r);
    return (
        <>
            <path d={sideFace(g)} fill={ctx.url('side')} />
            {fabric && <path d={sideFace(g)} fill={ctx.url('fabric')} />}
            <path d={topFace(g)} fill={top ?? ctx.url('top')} stroke={top ?? ctx.s.topA} stroke-width="3" stroke-linejoin="round" />
            <path d={`M${g.x0} ${g.yt}H${g.x1}V${g.yt + g.r}H${g.x0}Z`} fill={top ?? ctx.s.topB} />
            <path d={frontD} fill={front ?? ctx.url(fabric ? 'cloth' : 'body')} stroke={ctx.s.edge} stroke-width="0.8" />
            {fabric && <path d={frontD} fill={ctx.url('fabric')} />}
            <path d={frontD} fill={ctx.url('vshade')} />
            <path d={`M${g.x0 + g.r} ${g.yt + 0.6}H${g.x1 - g.r}`} stroke="#FFFFFF" stroke-opacity="0.18" stroke-width="1.2" />
        </>
    );
}

/** Отметки громкости на крышке: «−» слева, «+» справа. */
function VolumeMarks({ cy, half, color }: { cy: number; half: number; color: string }) {
    return (
        <g stroke={color} stroke-width="1.5" stroke-linecap="round">
            <line x1={120 - half - 6} y1={cy} x2={120 - half + 6} y2={cy} />
            <line x1={120 + half - 6} y1={cy} x2={120 + half + 6} y2={cy} />
            <line x1={120 + half} y1={cy - 3.5} x2={120 + half} y2={cy + 3.5} />
        </g>
    );
}

/** Перфорация по краю крышки — примета Лайт и Мини первого поколения. */
const Perforation = ({ cy, rx, ry, color }: { cy: number; rx: number; ry: number; color: string }) => (
    <ellipse cx="120" cy={cy} rx={rx} ry={ry} fill="none" stroke={color} stroke-width="1.6" stroke-dasharray="0.1 3.2" stroke-linecap="round" />
);

const VIOLET = '#9457FF';
const GLASS = '#121419';
const LED_WHITE = skinOf('graphite');

// ——— Модели ——————————————————————————————————————————————————————————————————————————————————————————————

/**
 * Наклейка-котик на фасаде Лайт 2 (из коробки): мордочка с ушками, усами и носом. Глаза — сам дисплей: часы и полоски
 * светят сквозь наклейку, как на фото; у Лайт 2 без часов глаза нарисованы.
 */
function CatSticker({ ctx }: { ctx: Ctx }) {
    const ink = ctx.s.light_ ? 'rgba(40, 30, 20, 0.75)' : 'rgba(8, 16, 40, 0.8)';
    return (
        <g>
            <path
                d="M64 152C57 132 55 113 61 97L70 69L95 84C109 80 131 80 145 84L170 69L179 97C185 113 183 132 176 152C150 160 90 160 64 152Z"
                fill={ctx.s.deep}
                fill-opacity="0.55"
                stroke={ink}
                stroke-width="1.3"
                stroke-linejoin="round"
            />
            <path d="M73 77L77 90L88 86Z M167 77L163 90L152 86Z" fill="#F2D45C" />
            {!ctx.clock && (
                <g fill={ink}>
                    <ellipse cx="100" cy="110" rx="7" ry="8" />
                    <ellipse cx="140" cy="110" rx="7" ry="8" />
                    <circle cx="102" cy="107" r="2" fill="#FFFFFF" />
                    <circle cx="142" cy="107" r="2" fill="#FFFFFF" />
                </g>
            )}
            <path d="M115 133H125L120 138Z" fill="#F49AB8" />
            <g fill="none" stroke={ink} stroke-width="1.2" stroke-linecap="round">
                <path d="M120 138C118 143 113 143 111 140M120 138C122 143 127 143 129 140" />
                <path d="M90 134L58 129M90 140L58 143M150 134L182 129M150 140L182 143" />
            </g>
        </g>
    );
}

const lite2: Model = {
    get title() {
        return tr.stations.modelTitle.lite2;
    },
    fallback: 'azure',
    sticker: true,
    draw: (ctx) => {
        const top = 52,
            rx = 90,
            ry = 22;
        return {
            shadow: 96,
            body: (
                <>
                    {/* Подставка: чуть уже корпуса, отделена бороздкой — как на фото. */}
                    <path d={cylinder(120, 150, 166, rx - 3, ry - 1)} fill={ctx.s.dark} />
                    <path d={cylinder(120, 150, 166, rx - 3, ry - 1)} fill={ctx.url('vshade')} />
                    <Drum ctx={ctx} top={top} bottom={150} rx={rx} ry={ry} />
                    <path d={`M${120 - rx} 150A${rx} ${ry} 0 0 0 ${120 + rx} 150`} fill="none" stroke="#000000" stroke-opacity="0.22" stroke-width="1.2" />
                    {ctx.sticker && (
                        <g clip-path={`url(#${ctx.id('lite2clip')})`}>
                            <clipPath id={ctx.id('lite2clip')}>
                                <path d={cylinder(120, top, 150, rx, ry)} />
                            </clipPath>
                            <CatSticker ctx={ctx} />
                        </g>
                    )}
                    <Lid ctx={ctx} cy={top} rx={rx} ry={ry} soft={10} />
                    <VolumeMarks cy={top} half={62} color={ctx.s.light_ ? 'rgba(0,0,0,0.35)' : 'rgba(255,255,255,0.75)'} />
                    <circle cx="120" cy={top - 15} r="1.2" fill="#000000" opacity="0.3" />
                    <circle cx="120" cy={top + 15} r="1.2" fill="#000000" opacity="0.3" />
                </>
            ),
            light: ringSpot(120, top, 15, 6, 0.36, 1.9),
            display: ctx.clock ? { cx: 120, cy: 108, w: 100, h: 32 } : undefined,
        };
    },
    glyph: (c) => (
        <>
            <path d="M3.5 8.5V17H24.5V8.5Z" fill={c.body} />
            <path d="M4.5 17V19A9.5 3 0 0 0 23.5 19V17Z" fill={c.body} opacity="0.75" />
            <ellipse cx="14" cy="17" rx="10.5" ry="3.2" fill={c.body} />
            <ellipse cx="14" cy="8.5" rx="10.5" ry="4" fill={c.top} />
            <ellipse cx="14" cy="8.5" rx="2" ry="0.9" fill={VIOLET} />
        </>
    ),
};

const lite1: Model = {
    get title() {
        return tr.stations.modelTitle.lite1;
    },
    fallback: 'lilac',
    draw: (ctx) => {
        const top = 64,
            rx = 92,
            ry = 22;
        return {
            shadow: 96,
            body: (
                <>
                    <defs>
                        <pattern id={ctx.id('ribs')} width="5" height="10" patternUnits="userSpaceOnUse">
                            <rect width="5" height="10" fill="#202227" />
                            <rect width="2" height="10" fill="#0E0F12" />
                        </pattern>
                    </defs>
                    {/* Тёмное рифлёное основание и цветной пластиковый верх. */}
                    <path d={cylinder(120, 138, 164, rx - 4, ry - 1)} fill={`url(#${ctx.id('ribs')})`} />
                    <path d={cylinder(120, 138, 164, rx - 4, ry - 1)} fill={ctx.url('vshade')} />
                    <Drum ctx={ctx} top={top} bottom={138} rx={rx} ry={ry} />
                    <Lid ctx={ctx} cy={top} rx={rx} ry={ry} soft={4} />
                    <Perforation cy={top} rx={rx - 9} ry={ry - 3} color={ctx.s.deep} />
                    <VolumeMarks cy={top} half={46} color="rgba(255,255,255,0.55)" />
                    <rect x="176" y="112" width="9" height="20" rx="2" fill="#1A1C20" />
                </>
            ),
            light: ringSpot(120, top, 5.5, 2.4, 1, 3.2),
        };
    },
    glyph: (c) => (
        <>
            <path d="M3 9V15H25V9Z" fill={c.body} />
            <path d="M3.5 15V18A10.5 3.2 0 0 0 24.5 18V15Z" fill="#1E2024" />
            <ellipse cx="14" cy="15" rx="11" ry="3.2" fill={c.body} />
            <ellipse cx="14" cy="9" rx="11" ry="4" fill={c.top} />
            <circle cx="14" cy="9" r="1" fill="#FFFFFF" />
        </>
    ),
};

const mini1: Model = {
    get title() {
        return tr.stations.modelTitle.mini1;
    },
    fallback: 'graphite',
    draw: (ctx) => {
        const top = 84,
            rx = 94,
            ry = 23;
        return {
            shadow: 98,
            body: (
                <>
                    <Drum ctx={ctx} top={top} bottom={162} rx={rx} ry={ry} fabric />
                    <Lid ctx={ctx} cy={top} rx={rx} ry={ry} fill={ctx.url('gloss')} />
                    <Perforation cy={top} rx={rx - 8} ry={ry - 2.5} color="rgba(0,0,0,0.75)" />
                    {/* Фиолетовое ушко на краю — примета первой Мини. */}
                    <path d="M196 84L206 82V104L196 106Z" fill={VIOLET} />
                    <path d="M196 84L206 82V86L196 88Z" fill="#FFFFFF" opacity="0.3" />
                </>
            ),
            light: ringSpot(120, top, 16, 5.2, 0.3, 1.8),
        };
    },
    glyph: (c) => (
        <>
            <path d="M2.5 10.5V16.5A11.5 3.6 0 0 0 25.5 16.5V10.5Z" fill={c.body} />
            <ellipse cx="14" cy="10.5" rx="11.5" ry="3.8" fill="#2A2D33" />
            <rect x="24.5" y="9.5" width="2" height="5" rx="0.6" fill={VIOLET} />
            <ellipse cx="14" cy="10.5" rx="2.6" ry="1" fill="none" stroke="#9AA3AD" stroke-width="0.8" />
        </>
    ),
};

const mini2: Model = {
    get title() {
        return tr.stations.modelTitle.mini2;
    },
    fallback: 'graphite',
    draw: (ctx) => {
        const top = 68,
            rx = 92,
            ry = 22;
        return {
            shadow: 96,
            body: (
                <>
                    <Drum ctx={ctx} top={top} bottom={162} rx={rx} ry={ry} fabric />
                    <Lid ctx={ctx} cy={top} rx={rx} ry={ry} fill={ctx.url('gloss')} />
                    <VolumeMarks cy={top} half={44} color="rgba(255,255,255,0.4)" />
                    {/* Кожаное ушко с «Я», перекинутое через край крышки. */}
                    <path d="M172 76C172 72 176 70 180 71L188 73V96C188 100 184 102 180 101C176 100 172 98 172 94Z" fill={ctx.s.deep} stroke="rgba(0,0,0,0.3)" stroke-width="0.8" />
                    <path d="M179 82V92M179 82C182 82 183 86 179 87L183 92" fill="none" stroke={ctx.s.light} stroke-opacity="0.6" stroke-width="1" stroke-linecap="round" />
                </>
            ),
            light: ringSpot(120, top, 14, 5, 0.38, 1.9),
            display: ctx.clock ? { cx: 116, cy: 116, w: 104, h: 34 } : undefined,
        };
    },
    glyph: (c) => (
        <>
            <path d="M2.5 9V16.5A11.5 3.6 0 0 0 25.5 16.5V9Z" fill={c.body} />
            <ellipse cx="14" cy="9" rx="11.5" ry="3.8" fill="#1E2024" />
            <ellipse cx="14" cy="9" rx="2.4" ry="0.9" fill="none" stroke={VIOLET} stroke-width="0.9" />
            <rect x="20.5" y="8.5" width="2.4" height="5" rx="1" fill={c.top} opacity="0.8" />
        </>
    ),
};

const mini3: Model = {
    get title() {
        return tr.stations.modelTitle.mini3;
    },
    fallback: 'graphite',
    draw: (ctx) => {
        const top = 60,
            rx = 90,
            ry = 22;
        return {
            shadow: 96,
            body: (
                <>
                    <Drum ctx={ctx} top={top} bottom={162} rx={rx} ry={ry} fabric />
                    {/* Мягкое плечо: ткань переходит на крышку без шва. */}
                    <Lid ctx={ctx} cy={top} rx={rx} ry={ry} soft={14} fabric />
                    <circle cx="70" cy={top} r="1.3" fill="#FFFFFF" opacity="0.4" />
                    <circle cx="170" cy={top} r="1.3" fill="#FFFFFF" opacity="0.4" />
                </>
            ),
            light: ringSpot(120, top, 13, 4.6, 0.36, 2),
            display: ctx.clock ? { cx: 120, cy: 114, w: 100, h: 32 } : undefined,
        };
    },
    glyph: (c) => (
        <>
            <path d="M3 8.5V17A11 3.6 0 0 0 25 17V8.5Z" fill={c.body} />
            <ellipse cx="14" cy="8.5" rx="11" ry="4" fill={c.top} />
            <ellipse cx="14" cy="8.5" rx="2.2" ry="0.9" fill="none" stroke={VIOLET} stroke-width="0.9" />
            <rect x="9" y="12.5" width="10" height="1.6" rx="0.8" fill="#FFFFFF" opacity="0.75" />
        </>
    ),
};

const mini3pro: Model = {
    get title() {
        return tr.stations.modelTitle.mini3pro;
    },
    fallback: 'graphite',
    draw: (ctx) => {
        const top = 50,
            rx = 82,
            ry = 20;
        return {
            shadow: 88,
            body: (
                <>
                    {/* Тёмная пластиковая база с тремя точками и тканевый цилиндр над ней. */}
                    <path d={cylinder(120, 128, 162, rx - 1, ry)} fill="#1C1E23" />
                    <path d={cylinder(120, 128, 162, rx - 1, ry)} fill={ctx.url('vshade')} />
                    <path d={`M${120 - rx + 1} 160A${rx - 1} ${ry} 0 0 0 ${120 + rx - 1} 160`} fill="none" stroke="#FFFFFF" stroke-opacity="0.08" stroke-width="3" />
                    {[-7, 0, 7].map((dx) => (
                        <circle key={dx} cx={120 + dx} cy={150} r="1.5" fill="#FFFFFF" opacity="0.7" />
                    ))}
                    <Drum ctx={ctx} top={top} bottom={128} rx={rx} ry={ry} fabric />
                    <path d={`M${120 - rx} 128A${rx} ${ry} 0 0 0 ${120 + rx} 128`} fill="none" stroke="#000000" stroke-opacity="0.35" stroke-width="1.2" />
                    <Lid ctx={ctx} cy={top} rx={rx - 4} ry={ry - 1} fill={ctx.url('gloss')} />
                </>
            ),
            // Свет — ободок по всему краю крышки.
            light: {
                region: `${ellipse(120, top, rx, ry)}${ellipse(120, top, rx - 6, ry - 2.2)}`,
                cx: 120,
                cy: top,
                r: rx * 1.1,
                halo: { rx: rx * 1.15, ry: ry * 1.9 },
            },
            display: ctx.clock ? { cx: 120, cy: 92, w: 96, h: 30 } : undefined,
        };
    },
    glyph: (c) => (
        <>
            <path d="M4 8V15H24V8Z" fill={c.body} />
            <path d="M4 15V18A10 3 0 0 0 24 18V15Z" fill="#1C1E23" />
            <ellipse cx="14" cy="15" rx="10" ry="3" fill={c.body} />
            <ellipse cx="14" cy="8" rx="10" ry="3.6" fill="#1E2024" stroke={VIOLET} stroke-width="1.3" />
        </>
    ),
};

/** Высокие кирпичи (Станция, Макс): круглая шайба-кнопка на крышке и светящийся ободок вокруг неё. */
function tower(key: 'station' | 'max', w: number, h: number, display?: (g: BoxGeo) => DisplayBox): Model {
    return {
        get title() {
            return tr.stations.modelTitle[key];
        },
        fallback: 'graphite',
        draw: (ctx) => {
            const g = boxGeo(w, h, 28, -16, 6);
            const c = topCenter(g);
            const capRx = 26,
                capRy = 8,
                capH = 6;
            return {
                shadow: 70,
                body: (
                    <>
                        <Box ctx={ctx} g={g} top={GLASS} />
                        <path d={topFace(g)} fill={ctx.url('gloss')} opacity="0.6" />
                        {/* Шайба: тёмный цилиндрик, на нём кнопки. */}
                        <path d={cylinder(c.x, c.y - capH, c.y, capRx, capRy)} fill="#1A1C21" />
                        <ellipse cx={c.x} cy={c.y - capH} rx={capRx} ry={capRy} fill="#3A3E46" />
                        <ellipse cx={c.x - 6} cy={c.y - capH - 2} rx={capRx * 0.6} ry={capRy * 0.5} fill={`url(#${ctx.id('spec')})`} opacity="0.35" />
                        <circle cx={c.x} cy={c.y - capH - 4.5} r="0.9" fill="#FFFFFF" opacity="0.6" />
                        <circle cx={c.x - 14} cy={c.y - capH} r="0.9" fill="#FFFFFF" opacity="0.6" />
                        <circle cx={c.x + 14} cy={c.y - capH} r="0.9" fill="#FFFFFF" opacity="0.6" />
                    </>
                ),
                light: {
                    region: `${ellipse(c.x, c.y + 0.5, capRx + 6, capRy + 2.6)}${ellipse(c.x, c.y, capRx, capRy)}`,
                    cx: c.x,
                    cy: c.y,
                    r: capRx * 1.4,
                    halo: { rx: capRx * 1.8, ry: capRy * 2.6 },
                },
                display: display?.(g),
                displaySkin: LED_WHITE,
            };
        },
        glyph: (c) => (
            <>
                <path d="M21 4L24 2.5V19.5L21 21Z" fill={c.body} opacity="0.65" />
                <rect x="8" y="4" width="13" height="17" rx="1.2" fill={c.body} />
                <path d="M8 4L11 2.5H24L21 4Z" fill="#14161A" />
                <ellipse cx="16" cy="3.3" rx="4.2" ry="1.3" fill="none" stroke={VIOLET} stroke-width="1.1" />
                {display && <rect x="11" y="12" width="7" height="1.6" rx="0.8" fill="#FFFFFF" opacity="0.85" />}
            </>
        ),
    };
}

const station = tower('station', 84, 126);
const max = tower('max', 90, 134, (g) => ({ cx: (g.x0 + g.x1) / 2, cy: g.yt + (g.yb - g.yt) * 0.56, w: 58, h: 20 }));

const station2: Model = {
    get title() {
        return tr.stations.modelTitle.station2;
    },
    fallback: 'graphite',
    draw: (ctx) => {
        const g = boxGeo(148, 86, 24, -16, 9);
        const c = topCenter(g);
        return {
            shadow: 100,
            body: (
                <>
                    <Box ctx={ctx} g={g} top={GLASS} />
                    <path d={topFace(g, 0.8)} fill={ctx.url('gloss')} />
                    {/* Пульт-таблетка посередине стеклянной крышки. */}
                    <path d={roundedRect(c.x - 26, c.y - 3.4, 52, 6.8, 3.4)} fill="#0A0B0E" stroke="#FFFFFF" stroke-opacity="0.12" stroke-width="0.8" />
                    <circle cx={c.x - 16} cy={c.y} r="0.9" fill="#FFFFFF" opacity="0.55" />
                    <circle cx={c.x + 16} cy={c.y} r="0.9" fill="#FFFFFF" opacity="0.55" />
                </>
            ),
            // Светится вся стеклянная крышка — от края к пульту.
            light: { region: `${topFace(g, 0.98)}${topFace(g, 0.8)}`, cx: c.x, cy: c.y, r: 90, halo: { rx: 96, ry: 18 } },
            display: { cx: (g.x0 + g.x1) / 2, cy: g.yt + 46, w: 70, h: 24 },
            displaySkin: LED_WHITE,
        };
    },
    glyph: (c) => (
        <>
            <path d="M23 9L25.5 7.5V19L23 20.5Z" fill={c.body} opacity="0.65" />
            <rect x="2.5" y="9" width="20.5" height="11.5" rx="1.6" fill={c.body} />
            <path d="M2.5 9L5 7.5H25.5L23 9Z" fill={VIOLET} />
            <rect x="9" y="13.5" width="8" height="1.6" rx="0.8" fill="#FFFFFF" opacity="0.8" />
        </>
    ),
};

const midi: Model = {
    get title() {
        return tr.stations.modelTitle.midi;
    },
    fallback: 'graphite',
    draw: (ctx) => {
        const g = boxGeo(104, 100, 24, -15, 9);
        const c = topCenter(g);
        return {
            shadow: 80,
            body: (
                <>
                    <Box ctx={ctx} g={g} top={GLASS} />
                    <path d={topFace(g, 0.94)} fill={ctx.url('gloss')} />
                    <circle cx={c.x - 26} cy={c.y + 2} r="1" fill="#FFFFFF" opacity="0.5" />
                    <circle cx={c.x + 26} cy={c.y - 2} r="1" fill="#FFFFFF" opacity="0.5" />
                </>
            ),
            light: ringSpot(c.x, c.y, 8.5, 3.4, 0.4, 2.4),
            display: ctx.clock ? { cx: (g.x0 + g.x1) / 2, cy: g.yt + 46, w: 68, h: 24 } : undefined,
        };
    },
    glyph: (c) => (
        <>
            <path d="M20.5 7L24 5V19L20.5 21Z" fill={c.body} opacity="0.65" />
            <rect x="5" y="7" width="15.5" height="14" rx="1.4" fill={c.body} />
            <path d="M5 7L8.5 5H24L20.5 7Z" fill="#14161A" />
            <circle cx="14.5" cy="6" r="1" fill={VIOLET} />
        </>
    ),
};

const duomax: Model = {
    get title() {
        return tr.stations.modelTitle.duomax;
    },
    fallback: 'graphite',
    draw: (ctx) => {
        // Анфас, как на фото: широкий экран ровно над тканевой базой, у подошвы базы — полоса света.
        const base = { x: 64, y: 100, w: 112, h: 70, r: 14 };
        const screen = { x: 26, y: 12, w: 188, h: 104, r: 10 };
        const glass = { x: screen.x + 7, y: screen.y + 7, w: screen.w - 14, h: screen.h - 14 };
        return {
            shadow: 80,
            body: (
                <>
                    <defs>
                        <linearGradient id={ctx.id('sky')} x1="0" y1="0" x2="0" y2="1">
                            <stop offset="0" stop-color="#3B5A86" />
                            <stop offset="0.5" stop-color="#8FA6C4" />
                            <stop offset="0.8" stop-color="#D8C3A8" />
                            <stop offset="1" stop-color="#C98A54" />
                        </linearGradient>
                        <clipPath id={ctx.id('glass')}>
                            <path d={roundedRect(glass.x, glass.y, glass.w, glass.h, 4)} />
                        </clipPath>
                    </defs>
                    {/* База: тканевый куб со скруглёнными углами. */}
                    <path d={roundedRect(base.x, base.y, base.w, base.h, base.r)} fill={ctx.url('cloth')} stroke={ctx.s.edge} stroke-width="0.8" />
                    <path d={roundedRect(base.x, base.y, base.w, base.h, base.r)} fill={ctx.url('fabric')} />
                    <path d={roundedRect(base.x, base.y, base.w, base.h, base.r)} fill={ctx.url('vshade')} />
                    {/* Тень экрана на базе. */}
                    <rect x={base.x} y={screen.y + screen.h - 2} width={base.w} height="10" fill="#000000" opacity="0.35" filter={ctx.url('glow')} />
                    {/* Экран: рамка, стекло с горами и облаками, камера сверху. */}
                    <path d={roundedRect(screen.x, screen.y, screen.w, screen.h, screen.r)} fill="#0B0C0F" stroke="#2C3038" stroke-width="1.6" />
                    <g clip-path={ctx.url('glass')}>
                        <rect x={glass.x} y={glass.y} width={glass.w} height={glass.h} fill={`url(#${ctx.id('sky')})`} />
                        <path d={`M${glass.x} ${glass.y + 78}L${glass.x + 40} ${glass.y + 52}L${glass.x + 62} ${glass.y + 60}L${glass.x + 90} ${glass.y + 34}L${glass.x + 118} ${glass.y + 58}L${glass.x + 150} ${glass.y + 48}L${glass.x + glass.w} ${glass.y + 70}V${glass.y + glass.h}H${glass.x}Z`} fill="#3A4252" opacity="0.85" />
                        <path d={`M${glass.x} ${glass.y + 84}C${glass.x + 50} ${glass.y + 74} ${glass.x + 120} ${glass.y + 90} ${glass.x + glass.w} ${glass.y + 78}V${glass.y + glass.h}H${glass.x}Z`} fill="#E9E4DC" opacity="0.55" />
                        <path d={roundedRect(glass.x + 52, glass.y + glass.h - 20, 74, 13, 4)} fill="#0B0C0F" opacity="0.55" />
                        <rect x={glass.x} y={glass.y} width={glass.w} height={glass.h} fill="#000000" opacity="0.18" />
                    </g>
                    <circle cx="120" cy={screen.y + 3.6} r="1.4" fill="#2C3038" />
                </>
            ),
            // Свет — полоса у подошвы базы.
            light: {
                region: roundedRect(base.x + 12, base.y + base.h - 7, base.w - 24, 4, 2),
                cx: 120,
                cy: base.y + base.h - 5,
                r: 56,
                halo: { rx: 64, ry: 10 },
            },
            display: { cx: 120, cy: 54, w: 92, h: 32 },
            displaySkin: LED_WHITE,
        };
    },
    glyph: (c) => (
        <>
            <rect x="8" y="13" width="12" height="9" rx="2" fill={c.body} />
            <rect x="2" y="2" width="24" height="14" rx="1.6" fill="#0B0C0F" />
            <rect x="3.5" y="3.5" width="21" height="11" rx="0.8" fill="#8FA6C4" />
            <rect x="10" y="20.6" width="8" height="1" rx="0.5" fill={VIOLET} />
        </>
    ),
};

const station3: Model = {
    get title() {
        return tr.stations.modelTitle.station3;
    },
    fallback: 'graphite',
    draw: (ctx) => {
        const g = boxGeo(90, 132, 14, -9, 10);
        const c = topCenter(g);
        const active = !ctx.offline && ctx.stage !== 'idle';
        return {
            shadow: 70,
            // Подсветка стены за колонкой — главная примета Станции 3 на фото.
            under: (
                <g class={ctx.offline ? undefined : active ? 'st2-anim st2-shimmer' : 'st2-anim st2-breathe'}>
                    <ellipse cx="120" cy="96" rx="76" ry="70" filter={ctx.url('wide')} style={{ fill: ctx.offline ? '#9AA3AD' : ctx.ring, opacity: active ? 0.55 : 0.28 }} />
                </g>
            ),
            body: <Box ctx={ctx} g={g} top={GLASS} />,
            light: { region: topFace(g, 0.9), cx: c.x, cy: c.y, r: 60, halo: { rx: 50, ry: 8 } },
            display: { cx: (g.x0 + g.x1) / 2, cy: g.yt + 46, w: 58, h: 20 },
            displaySkin: LED_WHITE,
        };
    },
    glyph: (c) => (
        <>
            <ellipse cx="14" cy="12" rx="11" ry="9" fill={VIOLET} opacity="0.35" />
            <rect x="8.5" y="3.5" width="11" height="18" rx="2.4" fill={c.body} />
            <rect x="10.5" y="8" width="7" height="1.6" rx="0.8" fill="#FFFFFF" opacity="0.9" />
        </>
    ),
};

const street: Model = {
    get title() {
        return tr.stations.modelTitle.street;
    },
    fallback: 'orange',
    draw: (ctx) => {
        // Туба лёжа: торец слева виден эллипсом, сверху — ремешок и панель кнопок.
        const top = 92,
            bottom = 166,
            h = bottom - top,
            r = h / 2;
        const x0 = 24,
            x1 = 214;
        const bodyD = `M${x0 + 10} ${top}H${x1 - r}A${r} ${r} 0 0 1 ${x1 - r} ${bottom}H${x0 + 10}Z`;
        const pill = { x: 104, y: top + 10, w: 82, h: 16 };
        return {
            shadow: 96,
            body: (
                <>
                    <defs>
                        <linearGradient id={ctx.id('tube')} x1="0" y1="0" x2="0" y2="1">
                            <stop offset="0" stop-color={ctx.s.light} />
                            <stop offset="0.35" stop-color={ctx.s.body} />
                            <stop offset="1" stop-color={ctx.s.deep} />
                        </linearGradient>
                    </defs>
                    <path d={`M60 ${top + 2}C58 ${top - 30} 92 ${top - 34} 96 ${top + 2}`} fill="none" stroke="#1D1F24" stroke-width="5" stroke-linecap="round" />
                    <path d={bodyD} fill={`url(#${ctx.id('tube')})`} stroke={ctx.s.edge} stroke-width="0.8" />
                    <path d={bodyD} fill={ctx.url('fabric')} />
                    <ellipse cx={x0 + 10} cy={top + r} rx="14" ry={r} fill={ctx.s.deep} />
                    <ellipse cx={x0 + 10} cy={top + r} rx="10" ry={r - 5} fill="#1D1F24" />
                    <ellipse cx={x0 + 8} cy={top + r - 8} rx="4" ry={r * 0.5} fill="#FFFFFF" opacity="0.08" />
                    <path d={roundedRect(pill.x, pill.y, pill.w, pill.h, pill.h / 2)} fill="#1A1C21" stroke="#FFFFFF" stroke-opacity="0.12" stroke-width="0.8" />
                    <g stroke="#FFFFFF" stroke-opacity="0.6" stroke-width="1.2" stroke-linecap="round">
                        <line x1={pill.x + 20} y1={pill.y + 8} x2={pill.x + 26} y2={pill.y + 8} />
                        <line x1={pill.x + 56} y1={pill.y + 8} x2={pill.x + 62} y2={pill.y + 8} />
                        <line x1={pill.x + 59} y1={pill.y + 5} x2={pill.x + 59} y2={pill.y + 11} />
                    </g>
                    <circle cx="74" cy={bottom - 18} r="5" fill="none" stroke={ctx.s.deep} stroke-width="1.4" />
                </>
            ),
            light: ringSpot(pill.x + pill.w / 2, pill.y + pill.h / 2, 5.4, 5.4, 0.42, 2.2),
        };
    },
    glyph: (c) => (
        <>
            <path d="M8 9C8 4 13 4 13 9" fill="none" stroke="#1D1F24" stroke-width="1.4" />
            <rect x="2" y="9" width="24" height="11" rx="5.5" fill={c.body} />
            <ellipse cx="4.5" cy="14.5" rx="2.2" ry="5.5" fill="#1D1F24" />
            <rect x="12" y="10.5" width="10" height="3" rx="1.5" fill="#1A1C21" />
            <circle cx="17" cy="12" r="0.9" fill={VIOLET} />
        </>
    ),
};

const bork: Model = {
    get title() {
        return tr.stations.modelTitle.bork;
    },
    fallback: 'white',
    draw: (ctx) => {
        const g = boxGeo(112, 132, 24, -15, 9);
        const c = topCenter(g);
        const shellH = 52;
        const frontW = g.x1 - g.x0;
        return {
            shadow: 82,
            body: (
                <>
                    <Box ctx={ctx} g={g} />
                    {/* Гладкая верхняя часть с экраном поверх ткани. */}
                    <path d={roundedRect(g.x0, g.yt, frontW, shellH, g.r)} fill={ctx.url('body')} />
                    <path d={`M${g.x1} ${g.yt}L${g.x1 + g.dx} ${g.yt + g.dy}V${g.yt + g.dy + shellH}L${g.x1} ${g.yt + shellH}Z`} fill={ctx.url('side')} />
                    <line x1={g.x0} y1={g.yt + shellH} x2={g.x1} y2={g.yt + shellH} stroke="#000000" stroke-opacity="0.25" stroke-width="1" />
                    <path d={roundedRect(g.x0 + 9, g.yt + 7, frontW - 18, shellH - 14, 5)} fill={GLASS} />
                    <path d={roundedRect(g.x0 + 9, g.yt + 7, frontW - 18, shellH - 14, 5)} fill="#E07A2E" opacity="0.12" />
                    <ellipse cx={c.x} cy={c.y} rx="24" ry="7" fill={ctx.s.light} stroke={ctx.s.edge} stroke-width="0.8" />
                </>
            ),
            light: { region: `${ellipse(c.x, c.y, 27, 8.4)}${ellipse(c.x, c.y, 24, 7)}`, cx: c.x, cy: c.y, r: 34, halo: { rx: 40, ry: 12 } },
            display: { cx: (g.x0 + g.x1) / 2, cy: g.yt + shellH / 2, w: 62, h: 22 },
            displaySkin: LED_WHITE,
        };
    },
    glyph: (c) => (
        <>
            <path d="M21.5 6L24.5 4.5V19.5L21.5 21Z" fill={c.body} opacity="0.7" />
            <rect x="6" y="6" width="15.5" height="15" rx="1.4" fill={c.body} stroke="rgba(0,0,0,0.25)" stroke-width="0.6" />
            <rect x="8" y="7.5" width="11.5" height="5" rx="1" fill="#121419" />
            <rect x="10" y="9.4" width="7" height="1.3" rx="0.6" fill="#F0A060" />
        </>
    ),
};

// ——— Сборка ——————————————————————————————————————————————————————————————————————————————————————————————

function drawing(model: Model): { Art: FunctionComponent<StationArtProps>; Glyph: FunctionComponent<StationGlyphProps> } {
    const Art = ({ skin, clock = true, sticker = false, ring, stage, offline, timeZone }: StationArtProps) => {
        const uid = useId().replace(/[^A-Za-z0-9_-]/g, '');
        const reduced = useReducedMotion();
        const time = useWallTime(timeZone);
        const s = skinOf(skin, model.fallback);
        const id = (name: string): string => `stm-${name}-${uid}`;
        const url = (name: string): string => `url(#${id(name)})`;
        const ctx: Ctx = { s, id, url, clock, sticker: sticker && !!model.sticker, ring, stage, offline };
        const drawn = model.draw(ctx);
        const showsTime = !!drawn.display && clock && !(stage === 'listen' || stage === 'speak');
        const label = tr.stations.artLabel(model.title, showsTime ? timeText(time) : null, offline ? tr.stations.offline : stageWord(stage));
        return (
            <Frame label={label}>
                <Defs id={id} skin={s} />
                <Shadow rx={drawn.shadow} blur={url('soft')} />
                {drawn.under}
                {drawn.body}
                <Light spot={drawn.light} ring={ring} stage={stage} offline={offline} uid={uid} reduced={reduced} />
                {drawn.display && <FrontDisplay box={drawn.display} time={time} stage={stage} skin={drawn.displaySkin ?? s} ring={ring} glow={url('glow')} clock={clock} offline={offline} />}
                {drawn.over}
            </Frame>
        );
    };
    const Glyph = ({ color, size = 28 }: StationGlyphProps) => (
        <svg aria-hidden="true" width={size} height={Math.round((size * 24) / 28)} viewBox="0 0 28 24">
            {model.glyph(colorInfo(color ?? model.fallback))}
        </svg>
    );
    return { Art, Glyph };
}

const ALL: Record<string, Model> = { lite2, lite1, mini1, mini2, mini3, mini3pro, station, max, station2, midi, duomax, station3, street, bork };

/** Рисунок и значок каждого семейства по id шлюза (gateway/src/stations/looks.rs). */
export const MODELS: Record<string, { title: string; Art: FunctionComponent<StationArtProps>; Glyph: FunctionComponent<StationGlyphProps> }> = Object.fromEntries(
    Object.entries(ALL).map(([id, model]) => [
            id,
            {
                get title() {
                    return model.title;
                },
                ...drawing(model),
            },
        ]),
);
