// The smallest character the engine takes: a round body with two eyes, a colour per mood and a light body only. It is a
// template for a new character (README, «Adding a character») and the demo's proof that the engine holds nothing of
// Deevi's: it has no look for `tender`, `wink` or `surprise` (they fall back) and no rich body (`auto` stays light).

import { type Body, type BodyHost, type MoodId, type PointerInput, type RigContext, defineCharacter } from '../src/index.ts';

interface SampleState {
    t: number;
    mood: MoodId;
    color: string;
    /** Height above the ground, in body radii. */
    jump: number;
    /** Eye height: 1 open, small when asleep or squinting. */
    eyes: number;
    lookX: number;
    lookY: number;
}

const LOOKS = {
    idle: { color: '#FBBF24' },
    listen: { color: '#FBBF24' },
    think: { color: '#F59E0B' },
    speak: { color: '#FBBF24' },
    success: { color: '#A3E635' },
    sad: { color: '#FDA4AF' },
    sleep: { color: '#FCF1DE' },
};
const INK = '#11100F';

function sampleRig(ctx: RigContext, first: MoodId) {
    let jumpV = 0;
    const s: SampleState = { t: 0, mood: first, color: LOOKS.idle.color, jump: 0, eyes: 1, lookX: 0, lookY: 0 };
    const colorOf = (id: MoodId): string => (LOOKS as Partial<Record<MoodId, { color: string }>>)[id]?.color ?? LOOKS.idle.color;
    s.color = colorOf(first);
    const hop = (v: number): void => {
        if (s.jump <= 0) jumpV = v;
    };
    return {
        setMood(id: MoodId, animate: boolean) {
            s.mood = id;
            s.color = colorOf(id);
            if (!animate) return;
            hop(id === 'success' ? 3.2 : 1.6);
            ctx.sound(id);
        },
        react(kind: 'tap' | 'wake') {
            hop(kind === 'wake' ? 2 : 2.6);
            ctx.sound(kind);
        },
        step(dt: number, pointer: PointerInput) {
            s.t += dt;
            if (s.jump > 0 || jumpV !== 0) {
                jumpV -= 12 * dt;
                s.jump = Math.max(0, s.jump + jumpV * dt);
                if (s.jump === 0) jumpV = 0;
            }
            const sleepy = s.mood === 'sleep' ? 0.12 : s.mood === 'think' ? 0.6 : s.mood === 'sad' ? 0.75 : 1;
            s.eyes += (sleepy - s.eyes) * Math.min(1, dt * 10);
            s.lookX = pointer.x * 4 + (s.mood === 'think' ? 6 : 0);
            s.lookY = -pointer.y * 3 + (s.mood === 'think' ? -6 : s.mood === 'sad' ? 4 : 0);
            return s;
        },
    };
}

function createSampleBody(host: BodyHost): Body<SampleState> {
    const NS = 'http://www.w3.org/2000/svg';
    const make = (name: string, attrs: Record<string, string | number>, parent: Element): SVGElement => {
        const node = document.createElementNS(NS, name);
        for (const [key, value] of Object.entries(attrs)) node.setAttribute(key, String(value));
        parent.appendChild(node);
        return node as SVGElement;
    };
    const svg = document.createElementNS(NS, 'svg');
    svg.setAttribute('viewBox', '-70 -90 140 154');
    svg.setAttribute('aria-hidden', 'true');
    svg.style.cssText = 'position:absolute;inset:0;width:100%;height:100%;display:block';
    const shadow = make('ellipse', { cx: 0, cy: 42, rx: 34, ry: 6, fill: 'rgba(0,0,0,0.18)' }, svg);
    shadow.style.display = host.shadow ? '' : 'none';
    const wave = make('circle', { r: 52, fill: 'none', 'stroke-width': 2, opacity: 0 }, svg);
    const body = make('g', {}, svg);
    const ball = make('circle', { r: 40 }, body);
    const eyes = [-14, 14].map((x) => make('ellipse', { cx: x, cy: -6, rx: 5, ry: 8, fill: INK }, body));
    host.container.insertBefore(svg, host.container.firstChild);

    return {
        draw(s) {
            const y = -s.jump * 40;
            body.setAttribute('transform', `translate(0 ${y})`);
            ball.setAttribute('fill', s.color);
            eyes.forEach((eye, i) => {
                eye.setAttribute('cx', String((i ? 14 : -14) + s.lookX));
                eye.setAttribute('cy', String(-6 + s.lookY));
                eye.setAttribute('ry', String(8 * s.eyes));
            });
            const talking = s.mood === 'listen' || s.mood === 'speak';
            wave.setAttribute('cy', String(y));
            wave.setAttribute('r', String(50 + Math.sin(s.t * 5) * 3));
            wave.setAttribute('opacity', talking ? '0.4' : '0');
            shadow.setAttribute('opacity', String(1 / (1 + s.jump)));
        },
        resize() {
            /* a fixed viewBox: the SVG keeps its shape */
        },
        setEffectColor(css) {
            wave.setAttribute('stroke', css);
        },
        dispose() {
            svg.remove();
        },
    };
}

export const sample = defineCharacter<SampleState>({
    id: 'sample',
    colors: { body: LOOKS.idle.color, ink: INK },
    looks: LOOKS,
    tunes: { idle: [520], success: [523, 784], sad: [392, 330], tap: [660, 880], wake: [440, 660] },
    rig: sampleRig,
    light: { create: createSampleBody },
});
