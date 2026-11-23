// Moved from panel/src/ui/divi.ts into the mascot library (mascot/README.md) on 23.11.2026; the panel imports it as
// `@duowave/mascot/mark`. It is DuoWave's logo, Deevi's face: it stays when the mascot (characters/index.ts CURRENT)
// changes.
//
// The live Deevi: the DuoWave mark in the panel's header (mark updated 13.11.2026: brand kit, logo/divi-mark.js; DIVI.md).
// A port of the site's divi-mark.js: the same poses, timings and curves, plus a stop when the mark leaves the page.
//
// At rest both eyes are open (capsules) and look ahead. The «Hello» scene, about 3.5 s: looks aside (eyes 7 to the side in
// 260 ms, 260 ms there, back in 220 ms) → blinks (80 + 130 ms) → 160 ms → winks (the right eye bends into an arch in 300 ms
// with overshoot) → 520 ms → opens the eye (260 ms) → 280 ms → smiles with both arches and hops (squats 140 ms,
// up by 16 in 190 ms, lands in 300 ms with overshoot) → 800 ms → looks ahead again (340 ms with overshoot).
// On load (`intro`): looks for 600 ms, then the scene. On hover over the logo (and on focus inside it) — the scene; while
// it plays, hovering does not restart it. With prefers-reduced-motion the mark stands still with open eyes.
//
// On its own (`idle`): `true` — after 10–16 s of nothing it blinks, about every third time it plays the scene.
// `'lively'` — Deevi of the sign-in and the connection wizard: every 3–6 s looks one way and the other and blinks; sometimes
// only blinks, rarely plays the scene. `cheer` — joy with a hop right on appearing: the speaker connected. While the
// page is hidden or the mark is off screen, it stands still.
//
// A server-rendered page (the sites) draws the still mark itself, so it is there before any script, and hands that
// <svg> to `mountDivi`, which brings it to life in place: viewBox 0 0 256 256, `<g data-divi="body">` holding the body
// path and `<g data-divi="l">`, `<g data-divi="r">` with one path each (DIVI_BODY, diviEyes('open'), DIVI_FILL). Such an
// <svg> can sit inside a bigger drawing. Colours come from `--divi-body` and `--divi-eye`, the brand's by default.

const NS = 'http://www.w3.org/2000/svg';
const BODY = 'M116 40H140A68 68 0 0 1 208 108V148A68 68 0 0 1 140 216H116A68 68 0 0 1 48 148V108A68 68 0 0 1 116 40Z';

/** Eyes 1.4×: centres at x 98 and 158, y 124; the arch is wider by a third of the added thickness. */
const K = 1.4;
const CY = 124;
const L = 98;
const R = 158;
const HS = 1 + 0.33 * (K - 1);

/** An eye is one cubic curve: a straight line when open, a circular arc when arched, so they blend smoothly. */
interface Eye {
    ox: number;
    x1: number;
    y1: number;
    ax: number;
    ay: number;
    bx: number;
    by: number;
    x2: number;
    y2: number;
    w: number;
    sy: number;
    /** Sideways shift: Deevi looks aside. */
    dx: number;
}
interface Body {
    sx: number;
    sy: number;
    ty: number;
}
interface Pose {
    l: Eye;
    r: Eye;
    b: Body;
}

const open = (x: number): Eye => ({
    ox: x,
    x1: x,
    y1: CY - 14,
    ax: x,
    ay: CY - 14 / 3,
    bx: x,
    by: CY + 14 / 3,
    x2: x,
    y2: CY + 14,
    w: 24 * K,
    sy: 1,
    dx: 0,
});

function arch(x: number): Eye {
    const a = 15 * HS;
    const h = 9;
    const y = CY + 5;
    const r = (a * a + h * h) / (2 * h);
    const cy = y - h + r;
    const aL = Math.atan2(y - cy, -a);
    const aR = Math.atan2(y - cy, a);
    const t = (4 / 3) * Math.tan((aR - aL) / 4) * r;
    return {
        ox: x,
        x1: x - a,
        y1: y,
        ax: x - a - t * Math.sin(aL),
        ay: y + t * Math.cos(aL),
        bx: x + a + t * Math.sin(aR),
        by: y - t * Math.cos(aR),
        x2: x + a,
        y2: y,
        w: 14 * K,
        sy: 1,
        dx: 0,
    };
}

const body = (change: Partial<Body> = {}): Body => ({ sx: 1, sy: 1, ty: 0, ...change });
const POSES = {
    listen: (): Pose => ({ l: open(L), r: open(R), b: body() }),
    /** Looks aside: both eyes shifted by `dx`. */
    look: (dx = 7): Pose => {
        const pose = POSES.listen();
        pose.l.dx = pose.r.dx = dx;
        return pose;
    },
    wink: (): Pose => ({ l: open(L), r: arch(R), b: body() }),
    happy: (): Pose => ({ l: arch(L), r: arch(R), b: body() }),
};

/** The mark's body on the 256×256 canvas. */
export const DIVI_BODY = BODY;

/** The logo's colours: the lime body and the dark eyes (brand kit, DIVI.md). */
export const DIVI_COLORS = { body: '#B6F36A', eye: '#0B0F0A' } as const;

/** The fills as CSS, so a page can recolour the mark (`--divi-body`, `--divi-eye`); the brand's colours otherwise. */
export const DIVI_FILL = { body: `var(--divi-body, ${DIVI_COLORS.body})`, eye: `var(--divi-eye, ${DIVI_COLORS.eye})` } as const;

/** A pose's eye as a line: path, width and sideways shift. */
export interface DiviEye {
    d: string;
    width: number;
    dx: number;
}

/**
 * Deevi's eyes in a pose, still: `open` — looks (listens), `look` — looks aside (thinks), `happy` —
 * smiles with arches (speaks). The same curves as the live mark.
 */
export function diviEyes(pose: 'open' | 'look' | 'happy'): [DiviEye, DiviEye] {
    const drawn = pose === 'happy' ? POSES.happy() : pose === 'look' ? POSES.look() : POSES.listen();
    const eye = (one: Eye): DiviEye => ({
        d: `M${one.x1} ${one.y1}C${one.ax} ${one.ay} ${one.bx} ${one.by} ${one.x2} ${one.y2}`,
        width: one.w,
        dx: one.dx,
    });
    return [eye(drawn.l), eye(drawn.r)];
}

const clone = (pose: Pose): Pose => ({ l: { ...pose.l }, r: { ...pose.r }, b: { ...pose.b } });

function mix<T extends object>(from: T, to: T, t: number): T {
    const out = { ...from } as Record<string, number>;
    for (const key of Object.keys(to)) {
        const a = (from as Record<string, number>)[key];
        const b = (to as Record<string, number>)[key];
        // The eye's centre does not drift: it jumps straight to the end position, as in divi-mark.js.
        out[key] = key === 'ox' ? b : a + (b - a) * t;
    }
    return out as T;
}
const lerp = (from: Pose, to: Pose, t: number): Pose => ({ l: mix(from.l, to.l, t), r: mix(from.r, to.r, t), b: mix(from.b, to.b, t) });

const ease = (t: number): number => (t < 0.5 ? 2 * t * t : 1 - Math.pow(-2 * t + 2, 2) / 2);
/** Ease-out-back with overshoot 1.70158 — for the landing, the eye bending into an arch and the gaze coming back. */
function back(t: number): number {
    const c1 = 1.70158;
    const c3 = c1 + 1;
    return 1 + c3 * Math.pow(t - 1, 3) + c1 * Math.pow(t - 1, 2);
}

function reduced(): boolean {
    try {
        return matchMedia('(prefers-reduced-motion: reduce)').matches;
    } catch {
        return false;
    }
}

export interface DiviOptions {
    /** On load: looks for 600 ms, then the «Hello» scene. */
    intro?: boolean;
    /** Where hovering (or focus) starts the scene: usually the whole logo (mark and wordmark). Without it — the mark itself. */
    hover?: Element;
    /** On its own: `true` — blinks now and then, `'lively'` — often looks around (see the top). */
    idle?: boolean | 'lively';
    /** On appearing — joy with a hop. */
    cheer?: boolean;
}

export interface DiviMark {
    /** Remove listeners and stop the animation: the mark left the page. */
    destroy: () => void;
}

/** The parts of a mark the page already drew (see the top), or null when `host` is not such an <svg>. */
function adopt(host: Element): { group: SVGGElement; eyes: Array<{ g: SVGGElement; path: SVGPathElement }> } | null {
    const part = (name: string): SVGGElement | null => host.querySelector<SVGGElement>(`[data-divi="${name}"]`);
    const group = host.tagName.toLowerCase() === 'svg' ? part('body') : null;
    const eyes = [part('l'), part('r')].map((g) => ({ g, path: g?.querySelector('path') ?? null }));
    if (!group || eyes.some((eye) => !eye.g || !eye.path)) return null;
    return { group, eyes: eyes as Array<{ g: SVGGElement; path: SVGPathElement }> };
}

/** Draw the live Deevi in `host` (size comes from host, the mark fills its width), or bring to life the still mark
 * `host` already is (an <svg>, see the top). */
export function mountDivi(host: Element, options: DiviOptions = {}): DiviMark {
    let pose = POSES.listen();
    let tween = 0;
    let busy = false;
    let alive = true;
    const timers = new Set<number>();

    const found = adopt(host);
    let group: SVGGElement;
    let eyes: Array<{ g: SVGGElement; path: SVGPathElement }>;
    if (found) ({ group, eyes } = found);
    else {
        const svg = document.createElementNS(NS, 'svg');
        svg.setAttribute('viewBox', '0 0 256 256');
        svg.setAttribute('aria-hidden', 'true');
        svg.style.cssText = 'display:block;width:100%;height:100%;overflow:visible';
        group = document.createElementNS(NS, 'g');
        group.setAttribute('data-divi', 'body');
        const shape = document.createElementNS(NS, 'path');
        shape.setAttribute('d', BODY);
        shape.style.fill = DIVI_FILL.body;
        group.appendChild(shape);
        eyes = ['l', 'r'].map((name) => {
            const g = document.createElementNS(NS, 'g');
            g.setAttribute('data-divi', name);
            const path = document.createElementNS(NS, 'path');
            path.setAttribute('fill', 'none');
            path.style.stroke = DIVI_FILL.eye;
            path.setAttribute('stroke-linecap', 'round');
            g.appendChild(path);
            group.appendChild(g);
            return { g, path };
        });
        svg.appendChild(group);
        host.replaceChildren(svg);
    }

    const set = (next: Pose): void => {
        pose = next;
        const { b } = next;
        // The body squats and hops from the bottom centre (128, 216).
        group.setAttribute('transform', `translate(0 ${b.ty}) translate(128 216) scale(${b.sx} ${b.sy}) translate(-128 -216)`);
        [next.l, next.r].forEach((eye, index) => {
            const { g, path } = eyes[index];
            g.setAttribute('transform', `translate(${eye.ox + eye.dx} ${CY}) scale(1 ${eye.sy}) translate(${-eye.ox} ${-CY})`);
            path.setAttribute('d', `M${eye.x1} ${eye.y1}C${eye.ax} ${eye.ay} ${eye.bx} ${eye.by} ${eye.x2} ${eye.y2}`);
            path.setAttribute('stroke-width', String(eye.w));
        });
    };

    const wait = (ms: number): Promise<void> =>
        new Promise((resolve) => {
            const id = window.setTimeout(() => {
                timers.delete(id);
                resolve();
            }, ms);
            timers.add(id);
        });

    const to = (target: Pose, ms: number, curve: (t: number) => number = ease): Promise<void> => {
        const from = clone(pose);
        const id = ++tween;
        return new Promise((resolve) => {
            // Timed by the frames themselves: the first frame is the start. A start taken from performance.now() can be
            // later than the next frame's stamp, and the overshooting curve would run backwards (eyes stretched into
            // lines). The site found this first (its PR «глаза ДиВи не растягиваются», 13.11).
            let start: number | null = null;
            const step = (now: number): void => {
                if (!alive || id !== tween) return resolve();
                start ??= now;
                const k = Math.min(1, (now - start) / ms);
                set(lerp(from, target, curve(k)));
                if (k < 1) requestAnimationFrame(step);
                else resolve();
            };
            requestAnimationFrame(step);
        });
    };

    /** Blink: the eyes squeeze to 8 % of their height in 80 ms and open in 130 ms. */
    const blink = async (): Promise<void> => {
        const was = clone(pose);
        const shut = clone(pose);
        shut.l.sy = shut.r.sy = 0.08;
        await to(shut, 80);
        await to(was, 130);
    };

    /** Rejoice: both arches, squat 140 ms, hop 16 px in 190 ms, land in 300 ms with overshoot. */
    const hop = async (): Promise<void> => {
        const squat = POSES.happy();
        const up = POSES.happy();
        squat.b = body({ sx: 1.07, sy: 0.9 });
        up.b = body({ sx: 0.97, sy: 1.05, ty: -16 });
        await to(squat, 140);
        await to(up, 190);
        await to(POSES.happy(), 300, back);
    };

    /** The «Hello» scene: looks aside → blinks → winks → opens the eye → smiles with a hop → looks ahead. */
    const greet = async (): Promise<void> => {
        if (busy || !alive) return;
        busy = true;
        await to(POSES.look(), 260);
        await wait(260);
        await to(POSES.listen(), 220);
        await blink();
        await wait(160);
        await to(POSES.wink(), 300, back);
        await wait(520);
        await to(POSES.listen(), 260);
        await wait(280);
        await hop();
        await wait(800);
        await to(POSES.listen(), 340, back);
        busy = false;
    };

    /** The speaker connected: joy with a hop at once, then looks ahead again. */
    const cheer = async (): Promise<void> => {
        if (busy || !alive) return;
        busy = true;
        await hop();
        await wait(800);
        await to(POSES.listen(), 340, back);
        busy = false;
    };

    let idleTimer = 0;
    let restartIdle: (() => void) | undefined;

    const hover = (): void => {
        if (busy || !alive) return;
        restartIdle?.();
        void greet();
    };

    /** Look one way, the other and straight again. */
    const lookAround = async (): Promise<void> => {
        const side = Math.random() < 0.5 ? -7 : 7;
        await to(POSES.look(side), 240);
        await wait(380);
        await to(POSES.look(-side), 320);
        await wait(380);
        await to(POSES.listen(), 260, back);
    };

    /** The mark is visible: a hidden variant of the scheme or a header scrolled off the edge stand still. */
    const onScreen = (): boolean => {
        const r = host.getBoundingClientRect();
        return r.width > 0 && r.bottom > 0 && r.top < window.innerHeight;
    };

    const idle = (lively: boolean): void => {
        const next = (): void => {
            window.clearTimeout(idleTimer);
            const ms = lively ? 3000 + Math.random() * 3000 : 10000 + Math.random() * 6000;
            if (alive) idleTimer = window.setTimeout(() => void tick(), ms);
        };
        const tick = async (): Promise<void> => {
            if (busy || !alive || document.hidden || !onScreen()) return next();
            const roll = Math.random();
            if (lively ? roll < 0.15 : roll < 0.34) await greet();
            else {
                busy = true;
                if (lively && roll < 0.7) {
                    await lookAround();
                    await wait(180);
                }
                await blink();
                busy = false;
            }
            next();
        };
        restartIdle = next;
        next();
    };

    set(POSES.listen());
    const target = options.hover ?? host;
    if (!reduced()) {
        if (options.cheer) void wait(250).then(cheer);
        else if (options.intro) void wait(600).then(greet);
        if (options.idle) idle(options.idle === 'lively');
        target.addEventListener('mouseenter', hover);
        target.addEventListener('focusin', hover);
    }

    return {
        destroy: () => {
            alive = false;
            tween++;
            window.clearTimeout(idleTimer);
            for (const id of timers) window.clearTimeout(id);
            timers.clear();
            target.removeEventListener('mouseenter', hover);
            target.removeEventListener('focusin', hover);
        },
    };
}

// The flat mascot (flat.ts) draws the same eyes as the mark: the curves, their centres and the blend between them.
export { CY as EYE_Y, L as EYE_LEFT, R as EYE_RIGHT, open as openEye, arch as archEye, mix as mixShape };
export type { Eye as EyeShape };
