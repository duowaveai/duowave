import type { OrbitKey } from './dash-model';

// Сцена дашборда, посчитанная под экран: колонка в центре орбиты, три узла полукругом по её верху — слух слева, мозг
// сверху, голос справа (как на столе шлюза в docs/design/roles/source/SpaceGateway.dc.html), внизу — капсула
// подключений.
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
/** Подпись слуха и голоса — до 122 точек шириной: половина. */
const LABEL_HALF = 61;
/** Карточка ответа под колонкой: две строки в полях — кто работает и что услышал или ответил. */
const CAPTION_H = 54;
const CAPTION_W = 196;
/** Кольцо колонки — на крышке: у Лайт 2 на высоте 52 из 200 рисунка. Туда влетает персонаж «Минимала». */
const RING_AT = 0.26;
/** Капсула «Подключено» внизу. */
const LINKS_H = 40;
const MIN_W = 390;
const MAX_W = 480;
/** Центр слуха и голоса — не ближе 65 точек к краю: подпись шириной до 122 остаётся на экране. */
const EDGE = 65;
const RX_MAX = 175;
const RY_MIN = 100;
const RY_MAX = 175;
const ART_MIN = 110;
const ART_MAX = 210;
/** Рисунок колонки — 240×200: высота — 5/6 ширины, корпус — 84% ширины. */
export const ART_RATIO = 5 / 6;
const BODY = 0.84;
/** Колонка растёт вместе с орбитой: самая большая — при самой высокой орбите. */
const ART_PER_RY = ART_MAX / RY_MAX;

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
    nodes: Record<OrbitKey, Point>;
    /** Колонка: левый верхний угол и ширина (высота — 5/6 ширины). */
    art: { left: number; top: number; size: number };
    caption: { top: number; width: number };
    /** Верх капсулы подключений; по горизонтали она по центру. */
    linksTop: number;
    /** Орбита и пути кометы по верхнему полукругу: от слуха к мозгу и от мозга к голосу. */
    track: string;
    toBrain: string;
    toVoice: string;
    /** Кольцо на крышке колонки: туда влетает персонаж «Минимала». */
    ring: Point;
}

const r = (value: number): number => Math.round(value * 10) / 10;

/**
 * Сцена под ширину экрана и высоту, что есть до панели вкладок (`room`; бесконечность — пока не измерили).
 * Сначала орбита: по ширине — сколько позволяют подписи слуха и голоса, по высоте — круг, если влезает. Потом
 * колонка с карточкой ответа: карточка — между подписями слуха и голоса, а если там тесно (узкий экран) — под ними.
 */
export function sceneOf(screenWidth: number, room: number): Scene {
    const scale = Math.min(1, screenWidth / MIN_W);
    const width = Math.min(MAX_W, Math.max(MIN_W, screenWidth));
    const tall = room / scale;
    const cx = width / 2;
    const rx = Math.min(RX_MAX, cx - EDGE);
    const captionW = CAPTION_W;
    const between = captionW / 2 + 6 <= rx - LABEL_HALF;

    // Орбита — круг, когда хватает высоты. Если карточка ляжет под подписи, орбита вытягивается вниз ровно настолько,
    // чтобы капсула осталась на её низу, под карточкой.
    const want = between ? rx : Math.max(rx, UNDER + 4 + CAPTION_H + 6 + LINKS_H / 2);
    const byRoom = (tall - TOP - LINKS_H / 2) / 2;
    const ry = Math.max(RY_MIN, Math.min(RY_MAX, want, byRoom));
    const cy = TOP + ry;
    const bottom = TOP + 2 * ry;
    const nodes: Record<OrbitKey, Point> = {
        brain: { x: cx, y: TOP },
        ear: { x: cx - rx, y: cy },
        voice: { x: cx + rx, y: cy },
    };

    // Колонка с карточкой — под кругом мозга; корпус не задевает круги слуха и голоса.
    const ceiling = TOP + CIRCLE / 2 - 6;
    const byWidth = (2 * (rx - CIRCLE / 2 - 8)) / BODY;
    const onOrbit = bottom - LINKS_H / 2;
    let size: number;
    let groupTop: number;
    if (between) {
        // Карточка между подписями: колонка с ней — посередине орбиты, над капсулой.
        const floor = onOrbit - 4;
        const byHeight = (floor - ceiling - CAPTION_H - 2) / ART_RATIO;
        size = Math.max(ART_MIN, Math.min(ART_MAX, ART_PER_RY * ry, byWidth, byHeight));
        const group = size * ART_RATIO + 2 + CAPTION_H;
        groupTop = Math.max(ceiling, Math.min(cy + 10 - group / 2, floor - group));
    } else {
        // Карточка под подписями слуха и голоса, колонка — прямо над ней.
        const captionTop = cy + UNDER + 4;
        const byHeight = (captionTop - 2 - ceiling) / ART_RATIO;
        size = Math.max(ART_MIN, Math.min(ART_MAX, ART_PER_RY * ry, byWidth, byHeight));
        groupTop = captionTop - 2 - size * ART_RATIO;
    }
    const captionTop = groupTop + size * ART_RATIO + 2;
    const linksTop = Math.max(onOrbit, captionTop + CAPTION_H + 6);
    const ring = { x: r(cx), y: r(groupTop + size * ART_RATIO * RING_AT) };
    return {
        width,
        height: Math.ceil(linksTop + LINKS_H),
        scale,
        nodes: Object.fromEntries(Object.entries(nodes).map(([key, point]) => [key, { x: r(point.x), y: r(point.y) }])) as Record<OrbitKey, Point>,
        art: { left: r(cx - size / 2), top: r(groupTop), size: r(size) },
        caption: { top: r(captionTop), width: captionW },
        linksTop: r(linksTop),
        track: `M${r(cx)} ${TOP}A${r(rx)} ${r(ry)} 0 1 1 ${r(cx)} ${r(bottom)}A${r(rx)} ${r(ry)} 0 1 1 ${r(cx)} ${TOP}Z`,
        toBrain: `M${r(nodes.ear.x)} ${r(cy)}A${r(rx)} ${r(ry)} 0 0 1 ${r(cx)} ${TOP}`,
        toVoice: `M${r(cx)} ${TOP}A${r(rx)} ${r(ry)} 0 0 1 ${r(nodes.voice.x)} ${r(cy)}`,
        ring,
    };
}
