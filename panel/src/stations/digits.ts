import type { WallTime } from './clock';

/** Семисегментные цифры светодиодных часов: одни на все модели, различаются размером и местом. */

/** Какие сегменты горят у цифры. */
export const SEGMENTS: Record<string, string> = {
    '0': 'abcdef',
    '1': 'bc',
    '2': 'abged',
    '3': 'abgcd',
    '4': 'fgbc',
    '5': 'afgcd',
    '6': 'afgedc',
    '7': 'abc',
    '8': 'abcdefg',
    '9': 'abcdfg',
    ' ': '',
};

/** Где и какого размера цифра: левый верхний угол, ширина, высота и толщина сегмента. */
export interface DigitBox {
    x: number;
    y: number;
    w: number;
    h: number;
    thickness: number;
    /** Зазор между сегментами; без него цифра сливается в ком. */
    gap: number;
}

const round = (n: number): number => Math.round(n * 10) / 10;

/** Цифра путями: каждый сегмент — длинный шестиугольник, как у светодиодных часов. */
export function digit(box: DigitBox, lit: string): { lit: string; all: string } {
    const { x, w, h, thickness, gap } = box;
    const y = box.y,
        k = thickness / 2;
    const xl = x + k,
        xr = x + w - k,
        yt = y + k,
        ym = y + h / 2,
        yb = y + h - k;
    const horizontal = (x1: number, x2: number, c: number): string =>
        `M${round(x1)} ${round(c)}L${round(x1 + k)} ${round(c - k)}L${round(x2 - k)} ${round(c - k)}L${round(x2)} ${round(c)}L${round(x2 - k)} ${round(c + k)}L${round(x1 + k)} ${round(c + k)}Z`;
    const vertical = (c: number, y1: number, y2: number): string =>
        `M${round(c)} ${round(y1)}L${round(c + k)} ${round(y1 + k)}L${round(c + k)} ${round(y2 - k)}L${round(c)} ${round(y2)}L${round(c - k)} ${round(y2 - k)}L${round(c - k)} ${round(y1 + k)}Z`;
    const segment: Record<string, string> = {
        a: horizontal(xl + gap, xr - gap, yt),
        g: horizontal(xl + gap, xr - gap, ym),
        d: horizontal(xl + gap, xr - gap, yb),
        f: vertical(xl, yt + gap, ym - gap),
        b: vertical(xr, yt + gap, ym - gap),
        e: vertical(xl, ym + gap, yb - gap),
        c: vertical(xr, ym + gap, yb - gap),
    };
    return { lit: [...lit].map((name) => segment[name]).join(''), all: [...'abcdefg'].map((name) => segment[name]).join('') };
}

/** Часы целиком: четыре цифры слева направо по x из `xs`, одного размера `box` (x берётся из xs). */
export function sevenSegment(xs: [number, number, number, number], box: Omit<DigitBox, 'x'>, time: WallTime): { lit: string; all: string; text: string } {
    // С ведущим нулём, как на самой колонке: 09:20, 00:15.
    const chars = [
        String(Math.floor(time.hours / 10)),
        String(time.hours % 10),
        String(Math.floor(time.minutes / 10)),
        String(time.minutes % 10),
    ];
    const digits = chars.map((char, index) => digit({ ...box, x: xs[index] }, SEGMENTS[char]));
    return {
        lit: digits.map((d) => d.lit).join(''),
        all: digits.map((d) => d.all).join(''),
        text: `${String(time.hours).padStart(2, '0')}:${String(time.minutes).padStart(2, '0')}`,
    };
}
