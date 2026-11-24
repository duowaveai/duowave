// Deevi's bubble (the designer's handoff 1.2.0, 24.11.2026): a clear shell in a pale, cooler shade of the mood's colour,
// and inside it three soft balls of light — the mood's colour (his own lime at rest), a lighter shade and a slightly
// warmer one — drifting slowly. The colours come from the mood's colour the same way for both bodies, so the flat one
// and the 3D one agree; the motion of the balls is the rig's (rig.ts `innerOf`).
//
// Colour maths, as in the handoff: r128 shifted hue and mixed the balls on the raw sRGB numbers, and mixed the shell
// toward white in linear light. Kept, so the shades are the designer's.

/** sRGB, each 0..1. */
export type RGB = [number, number, number];

const WHITE: RGB = [1, 1, 1];

const toLinear = (c: number): number => (c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4);
const toSrgb = (c: number): number => (c <= 0.0031308 ? c * 12.92 : 1.055 * c ** (1 / 2.4) - 0.055);
const clamp01 = (v: number): number => Math.max(0, Math.min(1, v));

/** `a` toward `b` by `t`, on the sRGB numbers. */
const mixRaw = (a: RGB, b: RGB, t: number): RGB => [0, 1, 2].map((i) => a[i] + (b[i] - a[i]) * t) as RGB;

/** `a` toward `b` by `t`, in linear light; the result in sRGB. */
export const mixLinear = (a: RGB, b: RGB, t: number): RGB =>
    [0, 1, 2].map((i) => toSrgb(toLinear(a[i]) + (toLinear(b[i]) - toLinear(a[i])) * t)) as RGB;

/** Hue, saturation and lightness shifted on the sRGB numbers, as three's `offsetHSL` did on raw colours. */
function offsetHsl([r, g, b]: RGB, dh: number, ds: number, dl: number): RGB {
    const max = Math.max(r, g, b);
    const min = Math.min(r, g, b);
    let h = 0;
    let s = 0;
    const l = (min + max) / 2;
    if (min !== max) {
        const d = max - min;
        s = l <= 0.5 ? d / (max + min) : d / (2 - max - min);
        if (max === r) h = (g - b) / d + (g < b ? 6 : 0);
        else if (max === g) h = (b - r) / d + 2;
        else h = (r - g) / d + 4;
        h /= 6;
    }
    h = (((h + dh) % 1) + 1) % 1;
    s = clamp01(s + ds);
    const L = clamp01(l + dl);
    if (s === 0) return [L, L, L];
    const p = L <= 0.5 ? L * (1 + s) : L + s - L * s;
    const q = 2 * L - p;
    const hue = (t: number): number => {
        const u = ((t % 1) + 1) % 1;
        if (u < 1 / 6) return q + (p - q) * 6 * u;
        if (u < 1 / 2) return p;
        if (u < 2 / 3) return q + (p - q) * 6 * (2 / 3 - u);
        return q;
    };
    return [hue(h + 1 / 3), hue(h), hue(h - 1 / 3)];
}

export interface BubbleColors {
    /** The three balls of light inside: the mood's colour, a lighter shade, a slightly warmer one. */
    inner: [RGB, RGB, RGB];
    /** The shell: a cooler, paler shade. */
    shell: RGB;
}

/** The bubble's colours for a mood's colour (`DeeviState.color`, sRGB 0–255). */
export function bubbleOf(color: readonly number[]): BubbleColors {
    const core: RGB = [color[0] / 255, color[1] / 255, color[2] / 255];
    return {
        inner: [core, mixRaw(core, WHITE, 0.35), offsetHsl(core, -0.05, 0.05, 0)],
        shell: mixLinear(offsetHsl(core, 0.12, -0.1, 0), WHITE, 0.55),
    };
}

/** The balls of light: radius and brightness, how far and how fast each drifts, and where in its path it starts. Units
 * as in rig.ts (the body is 2 × 2.2). */
export const INNER = [
    { r: 0.8, alpha: 1.0, ax: 0.07, ay: 0.06, speed: 0.6, phase: 0 },
    { r: 0.5, alpha: 0.8, ax: 0.32, ay: 0.28, speed: 0.9, phase: 2.1 },
    { r: 0.44, alpha: 0.8, ax: 0.32, ay: 0.3, speed: 1.15, phase: 4.2 },
] as const;

export const cssRgb = (c: RGB, alpha = 1): string =>
    `rgba(${Math.round(c[0] * 255)},${Math.round(c[1] * 255)},${Math.round(c[2] * 255)},${alpha})`;
