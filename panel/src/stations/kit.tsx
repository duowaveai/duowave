import { useEffect, useState } from 'preact/hooks';
import type { ComponentChildren } from 'preact';

import type { WallTime } from './clock';
import { COLORS } from './colors';
import { sevenSegment } from './digits';
import type { StationStage } from './types';

// Общие детали рисунков колонок в координатах 240×200: свет Алисы (кольцо, ободок, полоса), передний дисплей
// (часы или восемь полосок голоса), цвета корпуса и рамка рисунка с тенью. Модели (models.tsx) собирают из них
// корпус; анимация у всех одна, поэтому дашборд показывает «слушает / думает / говорит» одинаково на любой модели.

export const round = (n: number): number => Math.round(n * 10) / 10;

export const WORDS: Record<StationStage, string> = { idle: 'ждёт', listen: 'слушает', think: 'думает', speak: 'говорит' };

/** Смешать цвет с белым (k > 0) или чёрным (k < 0). */
export function shade(hex: string, k: number): string {
    const value = hex.replace('#', '');
    const channel = (at: number): number => {
        const base = parseInt(value.slice(at, at + 2), 16);
        const target = k > 0 ? 255 : 0;
        return Math.round(base + (target - base) * Math.abs(k));
    };
    return `#${[0, 2, 4].map((at) => channel(at).toString(16).padStart(2, '0')).join('')}`;
}

/** Насколько цвет светлый: на белом корпусе свет и цифры темнее, контур заметнее. */
export function luminance(hex: string): number {
    const value = hex.replace('#', '');
    const byte = (at: number): number => parseInt(value.slice(at, at + 2), 16);
    return (0.299 * byte(0) + 0.587 * byte(2) + 0.114 * byte(4)) / 255;
}

/** Цвета корпуса одной расцветки: свет слева, тень справа, тёмное дно, светодиоды. */
export interface Skin {
    light: string;
    /** Блик на ткани — слабее пластикового. */
    cloth: string;
    body: string;
    /** Основной тон ткани: чуть темнее пластика того же цвета. */
    mid: string;
    dark: string;
    deep: string;
    topA: string;
    topB: string;
    plate: string;
    edge: string;
    light_: boolean;
    led: string;
    ledGlow: string;
}

export function skinOf(id: string | undefined | null, fallback = 'graphite'): Skin {
    const c = (id && COLORS[id]) || COLORS[fallback];
    const light = luminance(c.body) > 0.68;
    return {
        light: shade(c.body, 0.3),
        cloth: shade(c.body, 0.1),
        body: c.body,
        mid: light ? c.body : shade(c.body, -0.1),
        dark: shade(c.body, -0.22),
        deep: shade(c.body, -0.45),
        topA: shade(c.top, 0.12),
        topB: shade(c.body, 0.16),
        plate: shade(c.body, light ? -0.55 : -0.6),
        edge: light ? 'rgba(60, 48, 30, 0.28)' : 'rgba(255, 255, 255, 0.16)',
        light_: light,
        led: light ? shade(c.body, -0.7) : '#F4FBFF',
        ledGlow: light ? shade(c.body, -0.4) : shade(c.top, 0.3),
    };
}

/** Эллипс путём: из него собираются кольца (два эллипса, правило evenodd) и ободки. */
export const ellipse = (cx: number, cy: number, rx: number, ry: number): string =>
    `M${round(cx - rx)} ${round(cy)}A${round(rx)} ${round(ry)} 0 1 0 ${round(cx + rx)} ${round(cy)}A${round(rx)} ${round(ry)} 0 1 0 ${round(cx - rx)} ${round(cy)}Z`;

export const roundedRect = (x: number, y: number, w: number, h: number, r: number): string =>
    `M${round(x + r)} ${round(y)}H${round(x + w - r)}A${r} ${r} 0 0 1 ${round(x + w)} ${round(y + r)}V${round(y + h - r)}A${r} ${r} 0 0 1 ${round(x + w - r)} ${round(y + h)}H${round(x + r)}A${r} ${r} 0 0 1 ${round(x)} ${round(y + h - r)}V${round(y + r)}A${r} ${r} 0 0 1 ${round(x + r)} ${round(y)}Z`;

/** Пользователь просил меньше движения: SMIL-анимации CSS-правило не гасит, поэтому их просто не рисуем. */
export function useReducedMotion(): boolean {
    const query = typeof window !== 'undefined' && window.matchMedia ? window.matchMedia('(prefers-reduced-motion: reduce)') : null;
    const [reduced, setReduced] = useState(query?.matches ?? false);
    useEffect(() => {
        if (!query) return;
        const update = (): void => setReduced(query.matches);
        query.addEventListener('change', update);
        return () => query.removeEventListener('change', update);
    }, []);
    return reduced;
}

/** Цвета Алисы: бирюза, синий, фиолетовый, сиреневый — так переливается свет настоящей колонки. */
const CYAN = '#3EE4FF',
    BLUE = '#4A6BFF',
    VIOLET = '#9457FF',
    ORCHID = '#D65CFF';

/**
 * Колесо цветов для каждого состояния. Первый цвет — того, кто отвечает (свой шлюз, Яндекс, агент): он остаётся в
 * переливе, чтобы и в движении было видно, чей ответ. Думая, колонка добавляет фиолетового, как настоящая.
 */
function wheelOf(stage: StationStage, ring: string): string[] {
    if (stage === 'think') return [ring, VIOLET, BLUE, ORCHID, VIOLET, CYAN];
    if (stage === 'listen') return [ring, CYAN, BLUE, ring, CYAN, VIOLET];
    return [ring, BLUE, ring, CYAN, ring, VIOLET];
}

const TURN: Record<StationStage, string> = { idle: '0s', listen: '2.6s', think: '1.3s', speak: '4.2s' };
const HALO_TURN: Record<StationStage, string> = { idle: '14s', listen: '3.4s', think: '1.8s', speak: '5s' };

/** Цвета ореола: те же, что у света, — и в покое, чтобы свечение было разноцветным, а не одного цвета. */
function haloOf(stage: StationStage, ring: string): string[] {
    if (stage === 'idle') return [ring, CYAN, BLUE, VIOLET, ORCHID];
    return wheelOf(stage, ring);
}

function wedge(cx: number, cy: number, r: number, from: number, to: number): string {
    const a = (from * Math.PI) / 180,
        b = (to * Math.PI) / 180;
    return `M${cx} ${cy}L${round(cx + r * Math.cos(a))} ${round(cy + r * Math.sin(a))}A${r} ${r} 0 0 1 ${round(cx + r * Math.cos(b))} ${round(cy + r * Math.sin(b))}Z`;
}

/** Вращающееся размытое колесо цветов — conic-gradient, которого в SVG нет. */
function Wheel({ cx, cy, r, colors, turn, reverse, reduced, blur }: { cx: number; cy: number; r: number; colors: string[]; turn: string; reverse?: boolean; reduced: boolean; blur?: string }) {
    const step = 360 / colors.length;
    return (
        <g filter={blur}>
            <g>
                {colors.map((color, index) => (
                    <path key={index} d={wedge(cx, cy, r, index * step - 0.5, (index + 1) * step + 0.5)} style={{ fill: color }} />
                ))}
                {!reduced && (
                    <animateTransform
                        attributeName="transform"
                        type="rotate"
                        from={`${reverse ? 360 : 0} ${cx} ${cy}`}
                        to={`${reverse ? 0 : 360} ${cx} ${cy}`}
                        dur={turn}
                        repeatCount="indefinite"
                    />
                )}
            </g>
        </g>
    );
}

/** Где светится модель: область света (путь, правило evenodd), его центр, ореол и — у кольца — серединка. */
export interface LightSpot {
    /** Сама светящаяся область: кольцо, ободок вокруг крышки, полоса. */
    region: string;
    cx: number;
    cy: number;
    /** Радиус колеса цветов: покрывает область с запасом. */
    r: number;
    /** Ореол вокруг света. */
    halo: { rx: number; ry: number };
    /** Серединка кольца: думая, в ней кружатся цвета. */
    core?: string;
}

/** Свет Алисы на модели: ждёт — ровный цвет отвечающего и дышит; слушает, думает, говорит — переливается. */
export function Light({ spot, ring, stage, offline, uid, reduced }: { spot: LightSpot; ring: string; stage: StationStage; offline?: boolean; uid: string; reduced: boolean }) {
    const id = (name: string): string => `stl-${name}-${uid}`;
    const url = (name: string): string => `url(#${id(name)})`;
    const { cx, cy, r } = spot;
    if (offline) {
        return <path d={spot.region} fill-rule="evenodd" fill="#9AA3AD" opacity="0.45" />;
    }
    const active = stage !== 'idle';
    const haloClass = stage === 'idle' ? 'st2-anim st2-breathe' : stage === 'speak' ? 'st2-anim st2-pulse' : stage === 'listen' ? 'st2-anim st2-shimmer' : 'st2-anim st2-glow';
    return (
        <g>
            <defs>
                <clipPath id={id('region')}>
                    <path d={spot.region} clip-rule="evenodd" />
                </clipPath>
                {spot.core && (
                    <clipPath id={id('core')}>
                        <path d={spot.core} />
                    </clipPath>
                )}
                <filter id={id('halo')} x="-100%" y="-100%" width="300%" height="300%">
                    <feGaussianBlur stdDeviation={Math.max(3, spot.halo.ry * 0.5)} />
                </filter>
                <filter id={id('mix')} x="-30%" y="-30%" width="160%" height="160%">
                    <feGaussianBlur stdDeviation={Math.max(1.2, r * 0.16)} />
                </filter>
            </defs>
            {/*
              Ореол — размытое колесо цветов Алисы, сплюснутое в эллипс света. Цвет того, кто отвечает, — один из его
              цветов: видно, чей ответ, но свечение не однотонное. Ждёт — колесо еле поворачивается, в работе — быстрее.
            */}
            <g class={haloClass}>
                <g filter={url('halo')} opacity={active ? 0.75 : 0.45}>
                    <g transform={`translate(${round(cx)} ${round(cy)}) scale(1 ${round((spot.halo.ry / spot.halo.rx) * 1000) / 1000})`}>
                        <Wheel cx={0} cy={0} r={spot.halo.rx * 0.8} colors={haloOf(stage, ring)} turn={HALO_TURN[stage]} reduced={reduced} />
                    </g>
                </g>
            </g>
            {active ? (
                <>
                    <g clip-path={url('region')} class={stage === 'listen' ? 'st2-anim st2-shimmer' : stage === 'speak' ? 'st2-anim st2-pulse' : undefined}>
                        <rect x={cx - r} y={cy - r} width={r * 2} height={r * 2} style={{ fill: ring }} />
                        <Wheel cx={cx} cy={cy} r={r} colors={wheelOf(stage, ring)} turn={TURN[stage]} reduced={reduced} blur={url('mix')} />
                        <path d={spot.region} fill-rule="evenodd" fill="#FFFFFF" opacity="0.18" />
                    </g>
                    {stage === 'think' && spot.core && (
                        <g clip-path={url('core')}>
                            <Wheel cx={cx} cy={cy} r={r} colors={[ORCHID, CYAN, VIOLET, BLUE, ring]} turn="1.9s" reverse reduced={reduced} blur={url('mix')} />
                            <path d={spot.core} fill="#FFFFFF" opacity="0.12" />
                        </g>
                    )}
                </>
            ) : (
                <g class="st2-anim st2-breathe">
                    <path d={spot.region} fill-rule="evenodd" style={{ fill: ring }} />
                    <path d={spot.region} fill-rule="evenodd" fill="#FFFFFF" opacity="0.22" />
                </g>
            )}
        </g>
    );
}

/** Кольцо света по центру крышки: внешний и внутренний эллипс, серединка — внутренний. */
export function ringSpot(cx: number, cy: number, rx: number, ry: number, thickness = 0.32, halo = 1.7): LightSpot {
    const inner = 1 - thickness;
    return {
        region: `${ellipse(cx, cy, rx, ry)}${ellipse(cx, cy, rx * inner, ry * inner)}`,
        core: ellipse(cx, cy, rx * inner, ry * inner),
        cx,
        cy,
        r: rx * 1.15,
        halo: { rx: rx * halo, ry: ry * halo },
    };
}

/** Передний дисплей: где он и какого цвета светодиоды. */
export interface DisplayBox {
    cx: number;
    cy: number;
    /** Ширина и высота цифр «12:35» целиком. */
    w: number;
    h: number;
}

/** Восемь полосок дисплея; вторые от краёв на настоящей колонке не загораются — здесь тоже. */
const BARS = [0, 1, 2, 3, 4, 5, 6, 7];
const QUIET = new Set([1, 6]);

/**
 * Дисплей на фасаде. Ждёт и думает — часы (если модель с часами). Слушает — полоски загораются от краёв к центру,
 * как волна внутрь; говорит — от центра наружу, как звук из колонки.
 */
export function FrontDisplay({ box, time, stage, skin, ring, glow, clock, offline }: { box: DisplayBox; time: WallTime; stage: StationStage; skin: Skin; ring: string; glow: string; clock: boolean; offline?: boolean }) {
    const voice = !offline && (stage === 'listen' || stage === 'speak');
    if (voice) {
        const pitch = box.w / 8;
        const bw = pitch * 0.78,
            bh = Math.min(box.h * 0.36, bw * 0.56);
        const x0 = box.cx - box.w / 2 + (pitch - bw) / 2;
        return (
            <g>
                {BARS.map((index) => {
                    const fromEdge = Math.min(index, 7 - index);
                    const delay = stage === 'listen' ? fromEdge * 0.13 : (3 - fromEdge) * 0.13;
                    const x = round(x0 + index * pitch),
                        y = round(box.cy - bh / 2);
                    if (QUIET.has(index)) return <rect key={index} x={x} y={y} width={round(bw)} height={round(bh)} rx={round(bh / 2)} fill={skin.led} opacity="0.05" />;
                    return (
                        <g key={index} class="st2-anim st2-bar" style={{ animationDelay: `${delay}s`, animationDuration: stage === 'listen' ? '1.05s' : '0.8s' }}>
                            <rect x={x - 1} y={y - 1} width={round(bw + 2)} height={round(bh + 2)} rx={round(bh / 2 + 1)} filter={glow} style={{ fill: ring }} opacity="0.9" />
                            <rect x={x} y={y} width={round(bw)} height={round(bh)} rx={round(bh / 2)} fill={skin.led} />
                        </g>
                    );
                })}
            </g>
        );
    }
    if (!clock) return null;
    const w = box.w / (4 + 3 * 0.34 + 0.55);
    const gap = w * 0.34,
        colon = w * 0.55;
    const x0 = box.cx - box.w / 2;
    const xs: [number, number, number, number] = [x0, x0 + w + gap, x0 + 2 * w + 2 * gap + colon, x0 + 3 * w + 3 * gap + colon];
    const thickness = Math.max(2, w * 0.24);
    const digits = sevenSegment(xs, { y: box.cy - box.h / 2, w, h: box.h, thickness, gap: thickness * 0.22 }, time);
    const colonX = x0 + 2 * w + 2 * gap + colon / 2;
    const dot = Math.max(2, thickness * 0.95);
    const dy = box.h * 0.22;
    const dim = stage === 'think' && !offline ? 0.55 : 1;
    return (
        <g opacity={dim}>
            <path d={digits.all} fill={skin.led} opacity="0.045" />
            <path d={digits.lit} fill={skin.ledGlow} opacity="0.8" filter={glow} />
            <path d={digits.lit} fill={skin.led} />
            <g class="st2-anim st2-blink">
                {[-dy, dy].map((offset) => (
                    <rect key={offset} x={round(colonX - dot / 2)} y={round(box.cy + offset - dot / 2)} width={round(dot)} height={round(dot)} rx={round(dot * 0.35)} fill={skin.led} />
                ))}
            </g>
        </g>
    );
}

/** Надпись времени для подписи рисунка. */
export const timeText = (time: WallTime): string => `${time.hours}:${String(time.minutes).padStart(2, '0')}`;

/** Общие для рисунка градиенты и фильтры: блик, размытие, ткань. Имена — через `id`. */
export function Defs({ id, skin }: { id: (name: string) => string; skin: Skin }) {
    return (
        <defs>
            <linearGradient id={id('body')} x1="0" y1="0" x2="1" y2="0">
                <stop offset="0" stop-color={skin.light} />
                <stop offset="0.3" stop-color={skin.body} />
                <stop offset="0.75" stop-color={skin.body} />
                <stop offset="1" stop-color={skin.dark} />
            </linearGradient>
            {/* Ткань светится меньше пластика: блик слабее, тень глубже. */}
            <linearGradient id={id('cloth')} x1="0" y1="0" x2="1" y2="0">
                <stop offset="0" stop-color={skin.light_ ? skin.light : skin.cloth} />
                <stop offset="0.35" stop-color={skin.mid} />
                <stop offset="0.75" stop-color={skin.mid} />
                <stop offset="1" stop-color={skin.deep} />
            </linearGradient>
            <linearGradient id={id('side')} x1="0" y1="0" x2="1" y2="0">
                <stop offset="0" stop-color={skin.dark} />
                <stop offset="1" stop-color={skin.deep} />
            </linearGradient>
            <linearGradient id={id('top')} x1="0" y1="0" x2="0" y2="1">
                <stop offset="0" stop-color={skin.topA} />
                <stop offset="1" stop-color={skin.topB} />
            </linearGradient>
            <linearGradient id={id('vshade')} x1="0" y1="0" x2="0" y2="1">
                <stop offset="0" stop-color="#FFFFFF" stop-opacity="0.12" />
                <stop offset="0.45" stop-color="#FFFFFF" stop-opacity="0" />
                <stop offset="1" stop-color="#000000" stop-opacity="0.24" />
            </linearGradient>
            <linearGradient id={id('gloss')} x1="0" y1="0" x2="0" y2="1">
                <stop offset="0" stop-color="#3A3F48" />
                <stop offset="1" stop-color="#0E1014" />
            </linearGradient>
            <radialGradient id={id('spec')} cx="0.5" cy="0.5" r="0.5">
                <stop offset="0" stop-color="#FFFFFF" stop-opacity="0.55" />
                <stop offset="1" stop-color="#FFFFFF" stop-opacity="0" />
            </radialGradient>
            {/* Ткань: мелкое плетение; на фото у тканевых моделей это первое, что отличает их от пластиковых. */}
            <pattern id={id('fabric')} width="3" height="3" patternUnits="userSpaceOnUse">
                <rect width="3" height="3" fill="#000000" opacity="0.06" />
                <circle cx="0.75" cy="0.75" r="0.62" fill="#000000" opacity="0.22" />
                <circle cx="2.25" cy="2.25" r="0.62" fill="#000000" opacity="0.22" />
                <circle cx="2.25" cy="0.75" r="0.4" fill="#FFFFFF" opacity="0.05" />
            </pattern>
            <filter id={id('soft')} x="-30%" y="-90%" width="160%" height="280%">
                <feGaussianBlur stdDeviation="5" />
            </filter>
            <filter id={id('glow')} x="-40%" y="-90%" width="180%" height="280%">
                <feGaussianBlur stdDeviation="2" />
            </filter>
            <filter id={id('wide')} x="-50%" y="-50%" width="200%" height="200%">
                <feGaussianBlur stdDeviation="14" />
            </filter>
        </defs>
    );
}

/** Рамка рисунка: коробка 6:5, SVG 240×200. */
export function Frame({ label, children }: { label: string; children: ComponentChildren }) {
    return (
        <div class="st2-frame">
            <svg role="img" aria-label={label} width="100%" height="100%" viewBox="0 0 240 200">
                {children}
            </svg>
        </div>
    );
}

/** Мягкая тень под колонкой: в светлой теме бледнее (переменная --station-shadow). */
export function Shadow({ rx, cy = 184, blur }: { rx: number; cy?: number; blur: string }) {
    return <ellipse cx="120" cy={cy} rx={rx} ry="8" fill="#000000" filter={blur} style={{ opacity: 'var(--station-shadow)' }} />;
}
