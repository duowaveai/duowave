import type { NodeKey } from './dash-model';

// Сцена дашборда, посчитанная под экран: колонка в центре орбиты, пять узлов на орбите, внизу — капсула подключений.
// Подписи и круги узлов всегда одного размера, чтобы читались; под высоту экрана меняются орбита и колонка в ней.
// Сцена занимает то, что остаётся до панели вкладок, пока страница вверху. Ниже RY_MIN орбита не сжимается —
// тогда остаток просто прокручивается. Уже 390 точек сцена уменьшается целиком, как раньше.

/** Подпись узла: две строки и поля. */
const LABEL_H = 42;
/** Между подписью и кругом узла. */
const GAP = 6;
export const CIRCLE = 52;
/** Центр круга мозга — верх орбиты. */
const TOP = LABEL_H + GAP + CIRCLE / 2;
/** От центра круга слуха или голоса до низа его подписи. */
const UNDER = CIRCLE / 2 + GAP + LABEL_H;
/** Карточка ответа под колонкой: две строки в полях — кто работает и что услышал или ответил. */
const CAPTION_H = 54;
/** Кольцо колонки — на крышке: у Лайт 2 на высоте 52 из 200 рисунка. Туда влетает персонаж «Минимала». */
const RING_AT = 0.26;
/** Капсула «Подключено» внизу. */
const LINKS_H = 40;
const MIN_W = 390;
const MAX_W = 480;
const RY_MIN = 100;
const RY_MAX = 175;
const ART_MIN = 110;
const ART_MAX = 210;
/** Рисунок колонки — 240×200: высота — 5/6 ширины, корпус — 84% ширины. */
export const ART_RATIO = 5 / 6;
const BODY = 0.84;
/** Колонка растёт вместе с орбитой: самая большая — при самой высокой орбите. */
const ART_PER_RY = ART_MAX / RY_MAX;
/** Характер и мышление — на 25° выше середины орбиты, слух и голос — на 20° ниже. */
const UP = (25 * Math.PI) / 180;
const DOWN = (20 * Math.PI) / 180;

export interface Point {
    x: number;
    y: number;
}

export interface Scene {
    /** Ширина и высота сцены в точках раскладки — до уменьшения на узком экране. */
    width: number;
    height: number;
    /** Уменьшение всей сцены на экране уже 390 точек. */
    scale: number;
    /** Центры кругов узлов: они лежат на орбите. */
    nodes: Record<NodeKey, Point>;
    /** Колонка: левый верхний угол и ширина (высота — 5/6 ширины). */
    art: { left: number; top: number; size: number };
    caption: { top: number; width: number };
    /** Верх капсулы подключений; по горизонтали она по центру. */
    linksTop: number;
    /** Орбита и пути кометы: от слуха к мозгу и от мозга к голосу. */
    track: string;
    toBrain: string;
    toVoice: string;
    /** Кольцо на крышке колонки: туда влетает персонаж «Минимала». */
    ring: Point;
}

const r = (value: number): number => Math.round(value * 10) / 10;

/**
 * Сцена под ширину экрана и высоту, что есть до панели вкладок (`room`; бесконечность — пока не измерили).
 * Сначала высота орбиты: самая большая, при которой всё влезает. Потом колонка: сколько позволяют круги узлов,
 * подпись под ней и сама орбита.
 */
export function sceneOf(screenWidth: number, room: number): Scene {
    const scale = Math.min(1, screenWidth / MIN_W);
    const width = Math.min(MAX_W, Math.max(MIN_W, screenWidth));
    const tall = room / scale;
    const cx = width / 2;
    // Слух и голос — не ближе 65 точек к краю: подпись шириной до 122 остаётся на экране.
    const side = Math.min(140, cx - 65);
    const rx = side / Math.cos(DOWN);
    const upSide = rx * Math.cos(UP);
    const lift = Math.sin(UP);
    const drop = Math.sin(DOWN);

    // Капсула ложится на низ орбиты, если подписи слуха и голоса кончаются выше неё, иначе — под ними.
    const byOrbit = (tall - TOP - LINKS_H / 2) / 2;
    const byLabels = (tall - TOP - UNDER - 6 - LINKS_H) / (1 + drop);
    const ry = Math.max(RY_MIN, Math.min(RY_MAX, byOrbit, byLabels));
    const cy = TOP + ry;
    const nodes: Record<NodeKey, Point> = {
        brain: { x: cx, y: TOP },
        persona: { x: cx - upSide, y: cy - ry * lift },
        think: { x: cx + upSide, y: cy - ry * lift },
        ear: { x: cx - side, y: cy + ry * drop },
        voice: { x: cx + side, y: cy + ry * drop },
    };
    const linksTop = Math.max(TOP + 2 * ry, nodes.ear.y + UNDER + 6 + LINKS_H / 2) - LINKS_H / 2;

    // Колонка с подписью — от круга мозга до подписей слуха и голоса; корпус не задевает круги характера и мышления.
    const ceiling = TOP + CIRCLE / 2 - 6;
    const floor = Math.min(nodes.ear.y + CIRCLE / 2 + GAP - 2, linksTop - 4);
    const byHeight = (floor - ceiling - CAPTION_H - 2) / ART_RATIO;
    const byWidth = (2 * (upSide - CIRCLE / 2 - 4)) / BODY;
    const size = Math.max(ART_MIN, Math.min(ART_MAX, ART_PER_RY * ry, byWidth, byHeight));
    const group = size * ART_RATIO + 2 + CAPTION_H;
    const groupTop = Math.max(ceiling, Math.min(cy + 10 - group / 2, floor - group));

    const bottom = TOP + 2 * ry;
    const ring = { x: r(cx), y: r(groupTop + size * ART_RATIO * RING_AT) };
    return {
        width,
        height: Math.ceil(linksTop + LINKS_H),
        scale,
        nodes: Object.fromEntries(Object.entries(nodes).map(([key, point]) => [key, { x: r(point.x), y: r(point.y) }])) as Record<NodeKey, Point>,
        art: { left: r(cx - size / 2), top: r(groupTop), size: r(size) },
        caption: { top: r(groupTop + size * ART_RATIO + 2), width: Math.round(Math.min(196, 2 * (side - CIRCLE / 2 - 4))) },
        linksTop: r(linksTop),
        track: `M${r(cx)} ${TOP}A${r(rx)} ${r(ry)} 0 1 1 ${r(cx)} ${r(bottom)}A${r(rx)} ${r(ry)} 0 1 1 ${r(cx)} ${TOP}Z`,
        toBrain: `M${r(nodes.ear.x)} ${r(nodes.ear.y)}A${r(rx)} ${r(ry)} 0 0 1 ${r(cx)} ${TOP}`,
        toVoice: `M${r(cx)} ${TOP}A${r(rx)} ${r(ry)} 0 0 1 ${r(nodes.voice.x)} ${r(nodes.voice.y)}`,
        ring,
    };
}
