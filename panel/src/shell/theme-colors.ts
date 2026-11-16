// Палитра из трёх цветов (docs/design/glass/THEMES.md, «Три цвета»): тема называет фон, текст и акцент режима, а
// остальные токены «Минимала» — карточки, линии, подложки, выбор, тени — пульт достраивает из них. Так тему можно
// попросить голосом («кремовая, с зелёным»): агенту хватает трёх цветов, а не восьмидесяти токенов.
// Цвета смысла — кто отвечает (лайм, янтарь, фиолет, камень), слои памяти, беда — не из трёх цветов: их берём у
// светлого или тёмного «Минимала» по яркости фона (DESIGN.md, The Who-Answers Rule).
// Файл чистый — без DOM: его читает themes.ts, а тот — vite.config.ts.

import type { Mode, Palette } from './themes';

/** Три цвета режима: фон, основной текст, акцент (главное действие, «Сказать»). `#rrggbb` или `#rgb`. */
export interface Anchors {
    bg: string;
    text: string;
    accent: string;
}

type Rgb = [number, number, number];

const HEX = /^#([0-9a-f]{3}|[0-9a-f]{6})$/i;

/** Цвет `#rgb` / `#rrggbb` — да или нет. */
export const isHex = (value: unknown): value is string => typeof value === 'string' && HEX.test(value);

function rgbOf(hex: string): Rgb {
    let digits = hex.slice(1);
    if (digits.length === 3) digits = digits.replace(/./g, (c) => c + c);
    return [0, 2, 4].map((at) => parseInt(digits.slice(at, at + 2), 16)) as Rgb;
}

function hexOf([r, g, b]: Rgb): string {
    return `#${[r, g, b].map((v) => Math.round(Math.min(255, Math.max(0, v))).toString(16).padStart(2, '0')).join('')}`.toUpperCase();
}

/** Цвет `a`, сдвинутый к `b` на долю `t` (0 — `a`, 1 — `b`). */
export function mix(a: string, b: string, t: number): string {
    const x = rgbOf(a);
    const y = rgbOf(b);
    return hexOf([0, 1, 2].map((i) => x[i] + (y[i] - x[i]) * t) as Rgb);
}

/** Цвет с прозрачностью. */
export function alpha(hex: string, a: number): string {
    const [r, g, b] = rgbOf(hex);
    return `rgba(${r}, ${g}, ${b}, ${a})`;
}

/** Яркость для глаза (WCAG): 0 — чёрный, 1 — белый. */
export function luminance(hex: string): number {
    const channel = (v: number): number => {
        const c = v / 255;
        return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
    };
    const [r, g, b] = rgbOf(hex).map(channel);
    return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

/** Контраст двух цветов (WCAG): 1 — одинаковые, 21 — чёрный на белом. Текст читается от 4,5. */
export function contrast(a: string, b: string): number {
    const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x);
    return (hi + 0.05) / (lo + 0.05);
}

/** Светлый ли фон: от него зависят тени, поверхности и то, чьи цвета смысла брать — светлого «Минимала» или тёмного. */
export const isLight = (bg: string): boolean => luminance(bg) > 0.35;

/** Акцент как текст на фоне: сам акцент, если читается, иначе — шаг за шагом к цвету текста, пока не прочтётся. */
function readableOn(color: string, bg: string, text: string): string {
    for (let t = 0; t <= 1; t += 0.1) {
        const tried = mix(color, text, t);
        if (contrast(tried, bg) >= 4.5) return tried;
    }
    return text;
}

/** Что не так с тремя цветами — словами; пусто — тема читается. Та же проверка в шлюзе (theme_form). */
export function anchorProblems(anchors: Anchors, mode: Mode): string[] {
    const problems: string[] = [];
    const ratio = contrast(anchors.text, anchors.bg);
    if (ratio < 4.5) problems.push(`colors.${mode}: текст на фоне не читается (контраст ${ratio.toFixed(1)}, нужно от 4,5)`);
    if (contrast(anchors.accent, anchors.bg) < 1.3) problems.push(`colors.${mode}: акцент сливается с фоном`);
    return problems;
}

/**
 * Токены режима из трёх цветов. `base` — палитра «Минимала» того же света, что фон (светлого или тёмного): из неё
 * цвета смысла и то, чего из трёх цветов не вывести. Тема может переписать любой токен своей `palette` поверх.
 */
export function paletteFromAnchors(anchors: Anchors, base: Palette): Palette {
    const { bg, text, accent } = anchors;
    const light = isLight(bg);
    // Ступени от фона к тексту: чем дальше от фона, тем заметнее. На тёмном нужно чуть больше, чтобы было видно.
    const step = (t: number): string => mix(bg, text, light ? t : t * 1.25);
    // То, что плавает (шит, меню, окно), — светлее фона: на светлом почти белое, на тёмном — на тон выше.
    const raised = light ? mix(bg, '#FFFFFF', 0.7) : step(0.08);
    const line = step(0.1);
    const onAccent = contrast('#0B0F0A', accent) >= contrast('#FFFFFF', accent) ? '#0B0F0A' : '#FFFFFF';
    const accentText = readableOn(accent, bg, text);
    return {
        ...base,
        bg,
        aurora: 'none',
        text,
        sub: alpha(text, light ? 0.58 : 0.62),
        mute: alpha(text, light ? 0.4 : 0.42),
        'glass-bg': step(0.035),
        'glass-border': line,
        'strong-bg': raised,
        'strong-border': line,
        'menu-bg': raised,
        divider: line,
        'row-line': step(0.06),
        track: step(0.08),
        'icon-bg': raised,
        'chip-bg': step(0.07),
        'field-bg': light ? mix(bg, '#FFFFFF', 0.85) : mix(bg, '#000000', 0.25),
        'hover-bg': step(0.03),
        accent,
        'accent-hi': light ? mix(accent, '#000000', 0.08) : mix(accent, '#FFFFFF', 0.15),
        'on-accent': onAccent,
        'accent-text': accentText,
        'talk-shadow': `0 6px 18px ${alpha(accent, light ? 0.32 : 0.22)}`,
        'talk-shadow-hi': `0 8px 26px ${alpha(accent, light ? 0.46 : 0.32)}`,
        'ink-bg': text,
        'ink-hi': mix(text, bg, 0.15),
        'ink-fg': bg,
        'pill-off-fg': alpha(text, light ? 0.58 : 0.62),
        'pill-off-bg': step(0.07),
        'seg-on-bg': light ? mix(bg, '#FFFFFF', 0.9) : step(0.16),
        'seg-on-fg': text,
        'seg-off-fg': alpha(text, light ? 0.58 : 0.62),
        'radio-on-bg': text,
        'radio-on-fg': bg,
        'radio-off': alpha(text, light ? 0.22 : 0.28),
        'label-bg': raised,
        'label-border': line,
        'on-badge': bg,
        orbit: step(0.13),
        'orbit-hi': base['orbit-hi'] ?? accent,
        'glow-rest': step(0.12),
        scrim: light ? alpha(text, 0.18) : 'rgba(0, 0, 0, 0.5)',
        grabber: step(0.13),
    };
}
