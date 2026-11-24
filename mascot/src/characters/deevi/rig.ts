// Deevi's motion, shared by both bodies: the pose per mood eased each frame, the hop and squash springs, the bouncy head
// tilt, breathing, blinking, the wink number, the sound waves. The bodies (flat.ts, scene3d.ts) only draw the state this
// returns, so they can't drift apart, and a swap from flat to 3D keeps him mid-hop.
//
// Units: one unit is 80 logo units (the 256 canvas). The body is 2 × 2.2 units with its centre 1.1 above the ground.

import type { MoodId } from '../../engine/moods.ts';
import type { PointerInput, Rig, RigContext } from '../../engine/types.ts';
import { INNER } from './bubble.ts';
import { LOOKS } from './looks.ts';

/** Numeric pose per mood; every value is eased toward its target each frame.
 *   ex, ey   eye width / height multipliers          tilt   eye tilt (outer corners down)
 *   lx, ly   gaze direction (-1..1)                  sqR    right-eye squint (1 = open)
 *   arc      both eyes bar -> arch (0..1)            wink   right eye only bar -> arch
 *   frown/oh/talk  mouth weights                     lean   head tilt, radians
 *   sink     resting squash                          dots   thinking dots
 *   ring     sound waves: 1 outward, -1 inward       blush  cheeks */
export interface Pose {
    ex: number;
    ey: number;
    tilt: number;
    lx: number;
    ly: number;
    sqR: number;
    arc: number;
    wink: number;
    frown: number;
    oh: number;
    talk: number;
    lean: number;
    sink: number;
    dots: number;
    ring: number;
    blush: number;
}

const REST: Pose = {
    ex: 1,
    ey: 1,
    tilt: 0,
    lx: 0,
    ly: 0,
    sqR: 1,
    arc: 0,
    wink: 0,
    frown: 0,
    oh: 0,
    talk: 0,
    lean: 0,
    sink: 0,
    dots: 0,
    ring: 0,
    blush: 0,
};
const KEYS = Object.keys(REST) as Array<keyof Pose>;
const pose = (change: Partial<Pose>): Pose => ({ ...REST, ...change });

export const POSES: Record<MoodId, Pose> = {
    idle: pose({}),
    tender: pose({ ly: 0.1, arc: 1, lean: 0.15, sink: 0.03, blush: 1 }),
    wink: pose({ lx: 0.25, ly: 0.05, lean: -0.1, wink: 1 }),
    sleep: pose({ ex: 1.05, ey: 0.1, ly: -0.25, lean: 0.05, sink: 0.06 }),
    listen: pose({ ex: 1.06, ey: 1.12, ly: 0.15, lean: 0.13, ring: -1 }),
    think: pose({ ey: 0.95, lx: 0.75, ly: 0.7, sqR: 0.42, lean: -0.07, dots: 1 }),
    speak: pose({ talk: 1, ring: 1 }),
    success: pose({ ly: 0.1, arc: 1 }),
    surprise: pose({ ex: 1.12, ey: 1.16, ly: 0.2, oh: 1 }),
    sad: pose({ ex: 0.95, ey: 0.8, tilt: 0.2, ly: -0.7, frown: 1, sink: 0.07 }),
};

/** What a body draws in one frame. */
export interface DeeviState {
    /** Seconds since he appeared. */
    t: number;
    /** The mood he is in (the bubble's inner light drifts faster when he thinks and dims when he sleeps). */
    mood: MoodId;
    /** The pose, eased toward the mood's. */
    pose: Pose;
    /** Head tilt in radians: the spring and the idle sway. */
    lean: number;
    /** Squash: positive is wider and lower (breathing, the resting sink and the landing). Scale x by 1 + 0.8·squash, y by 1 − squash. */
    squash: number;
    /** Height of the hop above the ground, in units. */
    jump: number;
    /** Eye height multiplier for blinking: 1 open, 0.08 shut. */
    blink: number;
    /** Gaze offset of the face, in units, y up. */
    lookX: number;
    lookY: number;
    /** The pointer across the container, -1..1: the 3D head turns toward it. */
    pointerX: number;
    /** How far the talking mouth is open now, 0..1. */
    talk: number;
    /** Sound waves: direction (1 out, -1 in, plus the tap's burst), phase, and the tap's burst 0..1. */
    ringDir: number;
    ringPhase: number;
    burst: number;
    /** Wink sparkles: 0 when none, else 0..1 through their life. */
    spark: number;
    /** The mood's colour, sRGB 0–255, eased toward the mood's: the light inside the bubble (bubble.ts). */
    color: [number, number, number];
    /** How far each ball of light inside the bubble has drifted along its path (bubble.ts INNER). */
    inner: [number, number, number];
    reduced: boolean;
}

// ---------- what both bodies compute the same way ----------

/** One eye this frame. `arc`: bar 0 → arch 1; `width`: stroke factor; `scaleY`: squash for squint and blink; `tilt`: radians, as in 3D (counter-clockwise). */
export function eyeOf(s: DeeviState, right: boolean): { arc: number; width: number; scaleY: number; tilt: number } {
    const p = s.pose;
    const squint = right ? p.sqR : 1;
    const arc = right ? Math.max(p.arc, p.wink) : p.arc; // a wink arches the right eye only
    return {
        arc,
        width: 1 + (p.ex - 1) * (1 - arc),
        scaleY: p.ey * squint * s.blink,
        tilt: (right ? 1 : -1) * p.tilt + (1 - squint) * -0.5,
    };
}

/** The «o» and talking mouth: half-width and half-height in units (0 when closed). */
export function mouthOf(s: DeeviState): { w: number; h: number } {
    const p = s.pose;
    return { w: 0.085 * p.oh + 0.1 * p.talk, h: 0.11 * p.oh + 0.03 * p.talk + 0.1 * s.talk };
}

/** Thinking dot `i` (0..2): radius in units, bouncing one after another. */
export function dotRadius(s: DeeviState, i: number): number {
    return (0.055 + i * 0.022) * s.pose.dots * (0.75 + 0.35 * Math.max(0, Math.sin(s.t * 4 - i * 0.9)));
}

/** Sound wave `i` (0..2): radius in units (the height is 1.08 of it) and opacity. */
export function ringOf(s: DeeviState, i: number): { radius: number; opacity: number } {
    const p = (s.ringPhase + i / 3) % 1;
    const q = s.ringDir >= 0 ? p : 1 - p;
    const opacity =
        s.reduced && s.burst <= 0 ? Math.abs(s.pose.ring) * 0.3 : Math.min(1, Math.abs(s.ringDir)) * Math.sin(p * Math.PI) * 0.55;
    return { radius: 1.3 + q * 0.95, opacity };
}

/** Ball of light `i` (0..2) inside the bubble: its centre in units from the body's centre (z toward the viewer), its
 * radius, and how bright it is (dimmer in sleep). It swells a little with the talking mouth. */
export function innerOf(s: DeeviState, i: number): { x: number; y: number; z: number; r: number; alpha: number } {
    const b = INNER[i];
    const t = s.inner[i];
    return {
        x: Math.sin(t + b.phase) * b.ax,
        y: Math.cos(t * 0.8 + b.phase) * b.ay,
        z: -0.15 + Math.sin(t * 0.6 + b.phase) * 0.12,
        r: b.r * (1 + s.pose.talk * 0.14 * Math.sin(s.t * 9 + i * 2)),
        alpha: b.alpha * (s.mood === 'sleep' ? 0.55 : 1),
    };
}

/** The ground shadow: how much of it shows (fades while he is up) and its size. */
export function shadowOf(s: DeeviState): { lift: number; spread: number } {
    return { lift: 1 / (1 + s.jump * 1.6), spread: 1 + s.squash * 0.8 - s.jump * 0.18 };
}

// ---------- the rig ----------

const rgb = (hex: string): [number, number, number] => {
    const n = parseInt(hex.slice(1), 16);
    return [(n >> 16) & 255, (n >> 8) & 255, n & 255];
};
const ease = (a: number, b: number, k: number, dt: number): number => a + (b - a) * (1 - Math.exp(-k * dt));
/** The squash spring's fastest push, per second: a hard landing (3.2) plus a mood's push (1.6) and a little over. */
const SQ_V_MAX = 5;

export function deeviRig(ctx: RigContext, first: MoodId): Rig<DeeviState> {
    const { reduced } = ctx;
    let mood = first;
    const cur: Pose = { ...POSES[first] };
    const target = rgb(LOOKS[first].color);
    const state: DeeviState = {
        t: 0,
        mood: first,
        pose: cur,
        lean: 0,
        squash: 0,
        jump: 0,
        blink: 1,
        lookX: 0,
        lookY: 0,
        pointerX: 0,
        talk: 0,
        ringDir: 0,
        ringPhase: 0,
        burst: 0,
        spark: 0,
        color: rgb(LOOKS[first].color),
        inner: [0, 0, 0],
        reduced,
    };
    let jumpV = 0;
    let sq = 0;
    let sqV = 0;
    let leanS = 0;
    let leanV = 0;
    let blinking = 0;
    let nextBlink = 1.5;
    let winkT = 0;
    let winkFired = false;
    let winkCycle = 0;
    const WP: Pose = { ...REST };

    /** Push the squash spring. Pushes add up, but only so far: several moods in one frame squash him once, not apart. */
    const kick = (v: number): void => {
        sqV = Math.max(-SQ_V_MAX, Math.min(SQ_V_MAX, sqV + v));
    };
    const hop = (v = 4.6): void => {
        if (state.jump < 0.02) {
            jumpV = v;
            kick(-1.2);
        }
    };
    const restartWink = (): void => {
        winkT = 0;
        winkFired = false;
        winkCycle = 0;
    };

    /** The wink as a little number: wind up, pop the wink with a hop and sparkles, hold, reset, repeat. */
    const winkPose = (dt: number): Pose => {
        winkT += dt;
        Object.assign(WP, POSES.wink);
        if (winkT < 0.34) Object.assign(WP, { wink: 0, ey: 1.15, sink: 0.08, lean: 0.08, lx: -0.25 });
        else if (winkT < 1.9) {
            if (!winkFired) {
                winkFired = true;
                hop(2.8);
                state.spark = 0.001;
                if (winkCycle === 0) ctx.sound('wink');
            }
            Object.assign(WP, { lean: -0.22, lx: 0.4 });
        } else if (winkT < 2.7) Object.assign(WP, { wink: 0, lean: 0, lx: 0 });
        else {
            winkT = 0;
            winkFired = false;
            winkCycle++;
            WP.wink = 0;
        }
        return WP;
    };

    return {
        setMood(id, animate) {
            mood = id;
            state.mood = id;
            const next = rgb(LOOKS[id].color);
            target[0] = next[0];
            target[1] = next[1];
            target[2] = next[2];
            if (id === 'wink') restartWink();
            if (!animate) return;
            if (id === 'success' || id === 'surprise' || id === 'tender') hop(id === 'success' ? 5.2 : id === 'tender' ? 3.4 : 4);
            else if (id !== 'sleep' && id !== 'sad' && id !== 'wink') kick(1.6);
            if (id !== 'wink') ctx.sound(id); // the wink chirps when it pops
        },

        react(kind) {
            if (kind === 'wake') {
                ctx.sound('wake');
                hop(3.2);
                return;
            }
            if (mood === 'wink') {
                restartWink(); // a tap = wink again
                return;
            }
            hop(4.6);
            state.burst = 1;
            ctx.sound('tap');
        },

        step(dt, pointer: PointerInput) {
            state.t += dt;
            const T = state.t;
            const P = mood === 'wink' ? winkPose(dt) : POSES[mood];
            for (const k of KEYS) cur[k] = ease(cur[k], P[k], k === 'wink' ? 30 : 9, dt);
            leanV += (-150 * (leanS - cur.lean) - 9 * leanV) * dt; // bouncy head tilt
            leanS += leanV * dt;
            if (state.spark > 0) {
                state.spark += dt / 0.9;
                if (state.spark >= 1) state.spark = 0;
            }
            for (let i = 0; i < 3; i++) state.color[i] = ease(state.color[i], target[i], 6, dt);
            // The light inside drifts slower in sleep, faster in thought.
            const drift = reduced ? 0.25 : mood === 'sleep' ? 0.35 : mood === 'think' ? 1.9 : 1;
            for (let i = 0; i < 3; i++) state.inner[i] += dt * INNER[i].speed * drift;

            // hop + squash spring
            if (state.jump > 0 || jumpV !== 0) {
                jumpV -= 17 * dt;
                state.jump += jumpV * dt;
                if (state.jump <= 0) {
                    kick(Math.min(3.2, -jumpV * 0.55));
                    state.jump = 0;
                    jumpV = 0;
                }
            }
            sqV += (-70 * sq - 9 * sqV) * dt;
            sq += sqV * dt;
            const breathe = Math.sin(T * (mood === 'sleep' ? 1.2 : 2.1)) * (reduced ? 0.004 : mood === 'sleep' ? 0.022 : 0.012);
            state.squash = sq + breathe + cur.sink;
            const sway = reduced ? 0 : Math.sin(T * 1.3) * 0.03 * (mood === 'think' ? 2.2 : mood === 'tender' ? 2.6 : 1);
            state.lean = leanS + sway;

            // blinking and gaze
            nextBlink -= dt;
            if (nextBlink <= 0) {
                blinking = 1;
                nextBlink = 2.2 + Math.random() * 3;
            }
            blinking = Math.max(0, blinking - dt * 7);
            state.blink = 1 - Math.sin(Math.min(1, blinking) * Math.PI) * 0.92;
            state.lookX = cur.lx * 0.07 + pointer.x * 0.05;
            state.lookY = cur.ly * 0.06 + pointer.y * 0.04;
            state.pointerX = pointer.x;

            state.talk = cur.talk * (0.35 + 0.65 * Math.abs(Math.sin(T * 11) * Math.sin(T * 3.7 + 1)));
            state.burst = Math.max(0, state.burst - dt * 1.1);
            state.ringDir = cur.ring + (state.burst > 0 ? state.burst : 0);
            state.ringPhase += dt * 0.55;
            return state;
        },
    };
}
