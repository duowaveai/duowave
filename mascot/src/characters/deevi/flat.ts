// Deevi's light body: the rig's state (rig.ts) drawn in SVG. It shows at once (no three.js to load), stands in where
// 3D is not wanted or not possible, and is what `auto` swaps for 3D a moment later — so it is framed like the 3D camera:
// the body takes the same share of the container's height, standing on the same line.
//
// The eyes are the mark's own curves (mark.ts): a capsule at rest, an arch when it smiles. A flat head can't turn, so the
// 3D turn toward the gaze shows as a slightly larger eye shift.

import { type Body, type BodyHost } from '../../engine/types.ts';
import { CHEEK, DEEVI_COLORS } from './looks.ts';
import { EYE_LEFT, EYE_RIGHT, EYE_Y, archEye, mixShape, openEye } from './mark.ts';
import { type DeeviState, dotRadius, eyeOf, mouthOf, ringOf, shadowOf } from './rig.ts';

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
    return (
        `<svg data-mascot-still="" aria-hidden="true" viewBox="${CX - 50} ${VIEW_Y - VIEW_H / 2} 100 ${VIEW_H}" preserveAspectRatio="xMidYMid meet" ` +
        `style="position:absolute;inset:0;width:100%;height:100%;display:block;overflow:visible">` +
        `${ground}<path d="${BODY}" fill="${DEEVI_COLORS.body}"/>${eyes}</svg>`
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
    const rings = [0, 1, 2].map(() => el('ellipse', { cx: CX, cy: CENTRE_Y, fill: 'none', 'stroke-width': 2.4, opacity: 0 }, svg));
    const rig = el('g', {}, svg); // hop + squash, pivot on the ground
    const head = el('g', {}, rig); // tilt, pivot in the centre
    const bodyPath = el('path', { d: BODY, fill: DEEVI_COLORS.body }, head);
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
            for (const ring of rings) ring.setAttribute('stroke', css);
            for (const dot of dots) dot.setAttribute('fill', css);
        },

        draw(s) {
            const p = s.pose;
            bodyPath.setAttribute('fill', `rgb(${s.color.map(Math.round).join(',')})`);
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
