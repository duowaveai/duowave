// Deevi's light body: the rig's state (rig.ts) drawn in SVG. It shows at once (no three.js to load), stands in where
// 3D is not wanted or not possible, and is what `auto` swaps for 3D a moment later — so it is framed like the 3D camera:
// the body takes the same share of the container's height, standing on the same line.
//
// The eyes are the mark's own curves (mark.ts): a capsule at rest, an arch when it smiles. A flat head can't turn, so the
// 3D turn toward the gaze shows as a slightly larger eye shift.
//
// The bubble (bubble.ts) drawn flat: a faint back wall, the three balls of light (radial gradients, clipped to the
// body), the front wall clear in the middle and pale toward the rim (a radial gradient standing in for the 3D fresnel),
// a thin rainbow rim and the two window glares where the 3D lights put them.

import { type Body, type BodyHost } from '../../engine/types.ts';
import { INNER, bubbleOf, cssRgb } from './bubble.ts';
import { CHEEK, DEEVI_COLORS, LOOKS } from './looks.ts';
import { EYE_LEFT, EYE_RIGHT, EYE_Y, archEye, mixShape, openEye } from './mark.ts';
import { type DeeviState, dotRadius, eyeOf, innerOf, mouthOf, ringOf, shadowOf } from './rig.ts';

const NS = 'http://www.w3.org/2000/svg';
const BODY = 'M116 40H140A68 68 0 0 1 208 108V148A68 68 0 0 1 140 216H116A68 68 0 0 1 48 148V108A68 68 0 0 1 116 40Z';
/** One rig unit in logo units. */
const U = 80;
/** The body's centre and the ground line on the 256 canvas. */
const CX = 128;
const CENTRE_Y = 128;
const GROUND = 216;
/** The 3D camera sees about this many logo units from top to bottom, centred on y 120 (see scene3d.ts): measured on
 * the rendered 3D body, 23.11.2026: in a 192 × 211 box the flat body is within 1.5 px of its height and 3 px of its
 * width (the 3D sides bulge a little in perspective). */
const VIEW_H = 412;
const VIEW_Y = 120;
const DEG = 180 / Math.PI;

const el = <K extends keyof SVGElementTagNameMap>(
    name: K,
    attrs: Record<string, string | number>,
    parent?: Element,
): SVGElementTagNameMap[K] => {
    const node = document.createElementNS(NS, name);
    for (const [key, value] of Object.entries(attrs)) node.setAttribute(key, String(value));
    parent?.appendChild(node);
    return node;
};

let uid = 0;

/** The front wall's opacity from the middle (0) to the rim (1): clear inside, pale at the edge, like the 3D fresnel. */
const WALL: Array<[number, number]> = [
    [0, 0.1],
    [0.62, 0.12],
    [0.78, 0.22],
    [0.9, 0.44],
    [0.97, 0.7],
    [1, 0.84],
];
/** A ball of light from its centre (0) to its edge (1): the 3D ball's `pow(cos, 1.1)` seen flat. */
const GLOW: Array<[number, number]> = [
    [0, 1],
    [0.3, 0.95],
    [0.5, 0.85],
    [0.7, 0.69],
    [0.85, 0.49],
    [0.95, 0.28],
    [1, 0],
];
/** The glares, where the 3D lights put them: up-left and, smaller, down-right. */
const GLARES = [
    { cx: 83, cy: 68, rx: 20, ry: 13, angle: -38, alpha: 0.75 },
    { cx: 176, cy: 178, rx: 9, ry: 6, angle: -40, alpha: 0.35 },
];

/** The bubble's static parts as markup: gradients (by `id`), back wall, clip, front wall, rim, glares. The balls of light
 * go between the walls (`inner`). */
function bubbleMarkup(id: string, inner: string, look = bubbleOf(rgbOf(LOOKS.idle.color))): { defs: string; body: string } {
    const stops = (list: Array<[number, number]>, color: string): string =>
        list.map(([at, alpha]) => `<stop offset="${at}" stop-color="${color}" stop-opacity="${alpha}"/>`).join('');
    const shell = cssRgb(look.shell);
    const defs =
        `<clipPath id="${id}-clip"><path d="${BODY}"/></clipPath>` +
        `<radialGradient id="${id}-wall" cx="0.5" cy="0.5" r="0.5">${stops(WALL, shell)}</radialGradient>` +
        look.inner
            .map((c, i) => `<radialGradient id="${id}-in${i}">${stops(GLOW, cssRgb(c))}</radialGradient>`)
            .join('') +
        `<radialGradient id="${id}-glare">${stops(
            [
                [0, 1],
                [0.5, 0.55],
                [1, 0],
            ],
            '#fff',
        )}</radialGradient>` +
        `<linearGradient id="${id}-rim" x1="0" y1="0" x2="1" y2="1">` +
        `<stop offset="0" stop-color="#ffd7f2"/><stop offset="0.35" stop-color="#d4f4ff"/>` +
        `<stop offset="0.7" stop-color="#fff4c4"/><stop offset="1" stop-color="#e3dcff"/></linearGradient>`;
    const glares = GLARES.map(
        (g) =>
            `<ellipse cx="${g.cx}" cy="${g.cy}" rx="${g.rx}" ry="${g.ry}" transform="rotate(${g.angle} ${g.cx} ${g.cy})" fill="url(#${id}-glare)" opacity="${g.alpha}"/>`,
    ).join('');
    const body =
        `<path d="${BODY}" fill="${shell}" fill-opacity="0.12"/>` +
        `<g clip-path="url(#${id}-clip)">${inner}</g>` +
        `<path d="${BODY}" fill="url(#${id}-wall)" stroke="url(#${id}-rim)" stroke-width="2.4" stroke-opacity="0.55"/>` +
        glares;
    return { defs, body };
}

const rgbOf = (hex: string): number[] => {
    const n = parseInt(hex.slice(1), 16);
    return [(n >> 16) & 255, (n >> 8) & 255, n & 255];
};

/** The balls of light at rest (idle, the start of their paths), as markup. */
function innerStill(id: string): string {
    return INNER.map((b, i) => {
        const x = CX + Math.sin(b.phase) * b.ax * U;
        const y = CENTRE_Y - Math.cos(b.phase) * b.ay * U;
        return `<circle cx="${x}" cy="${y}" r="${b.r * U}" fill="url(#${id}-in${i})" opacity="${b.alpha}"/>`;
    }).join('');
}

/** Deevi standing still in `idle`, as markup (Character.still): the same framing as the live body, whatever the box's
 * shape — the viewBox is narrow and `meet` scales it by the height, as the live body does; the sides overflow. */
export function stillFlat({ shadow = true }: { shadow?: boolean } = {}): string {
    const eyes = [EYE_LEFT, EYE_RIGHT]
        .map((x) => {
            const e = openEye(x);
            return `<path d="M${e.x1} ${e.y1}C${e.ax} ${e.ay} ${e.bx} ${e.by} ${e.x2} ${e.y2}" fill="none" stroke="${DEEVI_COLORS.ink}" stroke-linecap="round" stroke-width="${e.w}"/>`;
        })
        .join('');
    const ground = shadow
        ? `<defs><radialGradient id="deevi-still-shadow"><stop offset="0" stop-color="#000" stop-opacity="0.32"/><stop offset="1" stop-color="#000" stop-opacity="0"/></radialGradient></defs>` +
          `<ellipse cx="${CX}" cy="${GROUND + 2}" rx="92" ry="20" fill="url(#deevi-still-shadow)"/>`
        : '';
    // Several still frames on one page share these ids; they are the same drawing, so whichever comes first serves all.
    const bubble = bubbleMarkup('deevi-still', innerStill('deevi-still'));
    return (
        `<svg data-mascot-still="" aria-hidden="true" viewBox="${CX - 50} ${VIEW_Y - VIEW_H / 2} 100 ${VIEW_H}" preserveAspectRatio="xMidYMid meet" ` +
        `style="position:absolute;inset:0;width:100%;height:100%;display:block;overflow:visible">` +
        `<defs>${bubble.defs}</defs>${ground}${bubble.body}${eyes}</svg>`
    );
}

export function createFlat(host: BodyHost): Body<DeeviState> {
    const { container } = host;
    const id = `deevi-${++uid}`;

    const svg = el('svg', { 'aria-hidden': 'true', preserveAspectRatio: 'xMidYMid meet' });
    svg.style.cssText = 'position:absolute;inset:0;width:100%;height:100%;display:block;overflow:visible';
    const defs = el('defs', {}, svg);
    const shade = el('radialGradient', { id: `${id}-shadow` }, defs);
    el('stop', { offset: 0, 'stop-color': '#000', 'stop-opacity': 0.32 }, shade);
    el('stop', { offset: 1, 'stop-color': '#000', 'stop-opacity': 0 }, shade);

    const shadow = el('ellipse', { cx: CX, cy: GROUND + 2, rx: 92, ry: 20, fill: `url(#${id}-shadow)` }, svg);
    shadow.style.display = host.shadow ? '' : 'none';
    const rings = [0, 1, 2].map(() => el('ellipse', { cx: CX, cy: CENTRE_Y, fill: 'none', 'stroke-width': 3.4, opacity: 0 }, svg));
    const rig = el('g', {}, svg); // hop + squash, pivot on the ground
    const head = el('g', {}, rig); // tilt, pivot in the centre

    // The bubble: static markup (shared with the still frame), then the parts that change are looked up once.
    const bubble = bubbleMarkup(
        id,
        [0, 1, 2].map((i) => `<circle data-inner="${i}" fill="url(#${id}-in${i})"/>`).join(''),
    );
    defs.insertAdjacentHTML('beforeend', bubble.defs);
    const shell = el('g', {}, head);
    shell.innerHTML = bubble.body;
    const backWall = shell.firstElementChild as SVGPathElement;
    const balls = Array.from(shell.querySelectorAll<SVGCircleElement>('circle[data-inner]'));
    const wallStops = Array.from(defs.querySelectorAll(`#${id}-wall stop`));
    const innerStops = [0, 1, 2].map((i) => Array.from(defs.querySelectorAll(`#${id}-in${i} stop`)));
    let painted = '';
    let effect = DEEVI_COLORS.effect as string;
    /** The waves: the mood's colour nudged 15 % toward the effect colour, in linear light like the 3D body; plain mood
     * colour where CSS can't mix colours. */
    const mixes = typeof CSS !== 'undefined' && CSS.supports('color', 'color-mix(in srgb-linear, red 85%, blue)');
    const paint = (s: DeeviState): void => {
        const key = `${s.color.map(Math.round).join(',')}|${effect}`;
        if (key === painted) return;
        painted = key;
        const look = bubbleOf(s.color);
        const shellCss = cssRgb(look.shell);
        backWall.setAttribute('fill', shellCss);
        for (const stop of wallStops) stop.setAttribute('stop-color', shellCss);
        innerStops.forEach((stops, i) => {
            const c = cssRgb(look.inner[i]);
            for (const stop of stops) stop.setAttribute('stop-color', c);
        });
        const mood = `rgb(${s.color.map(Math.round).join(',')})`;
        const wave = mixes ? `color-mix(in srgb-linear, ${mood} 85%, ${effect})` : mood;
        for (const ring of rings) ring.style.stroke = wave;
    };
    const cheeks = [-1, 1].map((side) =>
        el(
            'ellipse',
            { cx: CX + side * 0.6 * U, cy: CENTRE_Y + 0.27 * U, rx: 0.15 * U, ry: 0.15 * U * 0.62, fill: CHEEK, opacity: 0 },
            head,
        ),
    );
    const face = el('g', {}, head); // gaze
    const eyes = [EYE_LEFT, EYE_RIGHT].map((x, index) => {
        const g = el('g', {}, face);
        const path = el('path', { fill: 'none', stroke: DEEVI_COLORS.ink, 'stroke-linecap': 'round' }, g);
        return { g, path, x, right: index === 1 };
    });
    const mouthY = CENTRE_Y + 0.47 * U;
    const mouth = el('ellipse', { cx: CX, cy: mouthY, rx: 0, ry: 0, fill: DEEVI_COLORS.ink }, face);
    const frownR = 0.13 * U;
    const frownY = mouthY + 0.08 * U;
    const frown = el(
        'path',
        {
            d: `M${CX - frownR} ${frownY}A${frownR} ${frownR} 0 0 1 ${CX + frownR} ${frownY}`,
            fill: 'none',
            stroke: DEEVI_COLORS.ink,
            'stroke-width': 0.08 * U,
            'stroke-linecap': 'round',
        },
        face,
    );
    const dots = [0, 1, 2].map((d) =>
        el('circle', { cx: CX + (0.55 + d * 0.27) * U, cy: CENTRE_Y - (1.1 + 0.32 + d * 0.16) * U, r: 0 }, head),
    );

    container.insertBefore(svg, container.firstChild);

    return {
        // Framing: the 3D camera keeps a fixed vertical view, so the width follows the container's shape.
        resize(w, h) {
            const vw = (VIEW_H * w) / h;
            svg.setAttribute('viewBox', `${CX - vw / 2} ${VIEW_Y - VIEW_H / 2} ${vw} ${VIEW_H}`);
        },

        setEffectColor(css) {
            effect = css;
            for (const dot of dots) dot.setAttribute('fill', css);
        },

        draw(s) {
            const p = s.pose;
            paint(s);
            balls.forEach((ball, i) => {
                const b = innerOf(s, i);
                ball.setAttribute('cx', String(CX + b.x * U));
                ball.setAttribute('cy', String(CENTRE_Y - b.y * U));
                ball.setAttribute('r', String(b.r * U));
                ball.setAttribute('opacity', String(b.alpha));
            });
            rig.setAttribute(
                'transform',
                `translate(0 ${-s.jump * U}) translate(${CX} ${GROUND}) scale(${1 + s.squash * 0.8} ${1 - s.squash}) translate(${-CX} ${-GROUND})`,
            );
            head.setAttribute('transform', `rotate(${-s.lean * DEG} ${CX} ${CENTRE_Y})`);
            face.setAttribute('transform', `translate(${s.lookX * U * 1.6} ${-s.lookY * U * 1.3})`);

            for (const eye of eyes) {
                const e = eyeOf(s, eye.right);
                const shape = mixShape(openEye(eye.x), archEye(eye.x), e.arc);
                eye.g.setAttribute(
                    'transform',
                    `translate(${eye.x} ${EYE_Y}) rotate(${-e.tilt * DEG}) scale(1 ${e.scaleY}) translate(${-eye.x} ${-EYE_Y})`,
                );
                eye.path.setAttribute(
                    'd',
                    `M${shape.x1} ${shape.y1}C${shape.ax} ${shape.ay} ${shape.bx} ${shape.by} ${shape.x2} ${shape.y2}`,
                );
                eye.path.setAttribute('stroke-width', String(shape.w * e.width));
            }
            for (const cheek of cheeks) cheek.setAttribute('opacity', String(0.5 * p.blush));

            const m = mouthOf(s);
            mouth.setAttribute('rx', String(m.w * U));
            mouth.setAttribute('ry', String(m.h * U));
            frown.setAttribute('opacity', String(Math.min(1, p.frown)));
            frown.setAttribute('transform', `translate(${CX} ${frownY}) scale(${p.frown + 0.0001}) translate(${-CX} ${-frownY})`);

            dots.forEach((dot, i) => dot.setAttribute('r', String(dotRadius(s, i) * U)));
            rings.forEach((ring, i) => {
                const r = ringOf(s, i);
                ring.setAttribute('rx', String(r.radius * U));
                ring.setAttribute('ry', String(r.radius * 1.08 * U));
                ring.setAttribute('cy', String(CENTRE_Y - s.jump * 0.5 * U));
                ring.setAttribute('opacity', String(r.opacity));
            });

            const sh = shadowOf(s);
            shadow.setAttribute('opacity', String(sh.lift));
            shadow.setAttribute('transform', `translate(${CX} ${GROUND}) scale(${sh.spread}) translate(${-CX} ${-GROUND})`);
        },

        dispose() {
            svg.remove();
        },
    };
}
