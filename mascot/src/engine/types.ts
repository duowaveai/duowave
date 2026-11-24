// The contracts between the engine (mount.ts), a character (characters/*) and a host (the panel, the sites).
//
//   host ──moods, taps──▶ engine ──moods, reactions, time──▶ character's rig ──state──▶ character's body ──▶ pixels
//
// The engine knows nothing about how a character looks or moves. A character knows nothing about pages, pointers,
// visibility or which body is drawing: it gets moods, reactions and time, and draws what its rig says.

import type { MoodId, Reaction } from './moods.ts';
import type { Tune } from './sound.ts';

/** Which body draws: `light` is always there and shows at once; `rich` is the character's heavy body, loaded on demand. */
export type BodyKind = 'light' | 'rich';

/** How a character looks in one mood. Hosts read the colour too, to paint the stage behind it. */
export interface MoodLook {
    /** Main colour in this mood (sRGB hex). */
    color: string;
}

/** A mood with the look a character resolved for it (after fallbacks). */
export interface Mood extends MoodLook {
    id: MoodId;
}

/** Sounds per mood and reaction; a mood without a tune is silent. */
export type Tunes = Partial<Record<MoodId | Reaction, Tune>>;

/** The pointer over the container. */
export interface PointerInput {
    /** -1 at the left edge to 1 at the right; 0 when the pointer is away. */
    x: number;
    /** -1 at the bottom edge to 1 at the top; 0 when the pointer is away. */
    y: number;
    /** While pressed and moved past the tap threshold: CSS px from where it went down. */
    drag: { dx: number; dy: number } | null;
}

/** What the engine gives a character's rig. */
export interface RigContext {
    /** The visitor asked for less motion (prefers-reduced-motion): keep breathing and swaying small. */
    reduced: boolean;
    /** Play this character's tune for a mood or reaction; silent unless the host turned sound on. */
    sound(key: MoodId | Reaction): void;
}

/**
 * A character's motion and state, shared by its bodies. The engine feeds it moods, reactions and time; the bodies draw
 * what `step` returns. It lives as long as the mascot, so swapping bodies keeps the motion going.
 */
export interface Rig<S> {
    /** `id` is always a mood the character has a look for: the engine resolved fallbacks. */
    setMood(id: MoodId, animate: boolean): void;
    /** A tap (after the host's `onTap` let it through), or waking from sleep (the engine already set `listen`). */
    react(kind: Reaction): void;
    /** Advance by `dt` seconds (0 for the first frame) and return the state to draw. */
    step(dt: number, pointer: PointerInput): S;
}

/** What the engine gives a body it creates. */
export interface BodyHost {
    /** The body puts its drawing inside, absolutely positioned (the engine made the container `position: relative`). */
    container: HTMLElement;
    /** Draw the soft ground shadow. */
    shadow: boolean;
    /** Call when the body can no longer draw (WebGL lost its context): the engine puts the light body in its place. */
    lost(): void;
}

/** One way to draw a character: SVG, WebGL, a sprite sheet. Stateless apart from its own view (camera, drag turn). */
export interface Body<S> {
    draw(state: S, dt: number, pointer: PointerInput): void;
    /** The container's size in CSS px, whenever it changes (never 0). */
    resize(width: number, height: number): void;
    /** Colour for effects (waves, dots, sparkles). */
    setEffectColor(css: string): void;
    /** Take the drawing out and free what it holds. */
    dispose(): void;
}

/** Makes a body. May throw (no WebGL): the engine keeps the light body then. */
export type BodyFactory<S> = (host: BodyHost) => Body<S>;

export interface BodySpec<S> {
    create: BodyFactory<S>;
    /** The body turns when dragged: the engine lets the page scroll only vertically over it (`touch-action: pan-y`). */
    drag?: boolean;
}

export interface RichBodySpec<S> {
    /** Fetch the body's code (a dynamic import, its own chunk). */
    load(): Promise<BodyFactory<S>>;
    drag?: boolean;
    /** `auto` keeps the light body in a container smaller than this on either side, CSS px. Default 96. */
    minSize?: number;
    /** `auto` keeps the light body while this many rich bodies run on the page. Default 4: a page has room for about 16 WebGL contexts. */
    maxLive?: number;
}

/**
 * A character: everything that makes one mascot different from another. `S` is the state its rig hands its bodies.
 * Write one with `defineCharacter` (README, «Adding a character»).
 */
export interface Character<S = unknown> {
    /** Stable id, like `deevi`. Hosts key their words (the name a screen reader says) by it. */
    id: string;
    /**
     * The character's own colours: the main one, the ink of its features and, optionally, the colour of its effects
     * (waves, dots, sparkles) when the host names none. Without it the effects take the container's CSS `color`.
     */
    colors: { body: string; ink: string; effect?: string };
    /** Looks per mood. `idle` is required; the rest fall back (engine/moods.ts FALLBACK). */
    looks: { idle: MoodLook } & Partial<Record<MoodId, MoodLook>>;
    tunes?: Tunes;
    /** Make the rig for one mascot, starting in `first` (a mood it has a look for). */
    rig(ctx: RigContext, first: MoodId): Rig<S>;
    /** The light body: always available, shown at once. */
    light: BodySpec<S>;
    /** The rich body, if the character has one. */
    rich?: RichBodySpec<S>;
    /**
     * The light body standing still in `idle`, as SVG markup framed exactly like the live light body, for a page
     * rendered on a server: it shows before any script runs, and the host swaps it for the live mascot (which then
     * moves into its mood). Marked `data-mascot-still`. Optional; the engine itself never calls it.
     */
    still?(options?: { shadow?: boolean }): string;
}

export interface MascotOptions {
    /** Who to draw. Default: the character DuoWave shows now (characters/index.ts CURRENT). */
    character?: Character;
    /** First mood; `idle` when unknown. */
    mood?: MoodId;
    /** Tap to react, eyes follow the pointer, Enter and Space tap; a body that turns also turns when dragged. Default true. */
    interactive?: boolean;
    /** Chirps on mood changes and taps. Browsers allow sound only after a user gesture. Default false. */
    sound?: boolean;
    /** Soft ground shadow. Default true. */
    shadow?: boolean;
    /**
     * Colour of the effects. When not given: the container's `--mascot-effect` CSS property (it inherits, so a theme can
     * set it once), then the character's own (`colors.effect`), then the container's CSS `color`.
     */
    effectColor?: string;
    /** After every mood change (not the first mood); a tap waking a sleeping mascot changes the mood too. */
    onMoodChange?: (id: MoodId, mood: Mood) => void;
    /** A tap. Return `false` to skip the character's reaction (and waking up). */
    onTap?: (id: MoodId) => boolean | void;
    /**
     * `auto` (default): the light body at once, then the rich one when it loads — unless the character has none, the
     * container is small or too many rich bodies are on the page. `rich` tries it whatever the size; `light` never loads it.
     */
    body?: 'auto' | BodyKind;
}

/** One mascot on the page. The host never needs to know which character or body it got. */
export interface Mascot {
    readonly character: Character;
    /** The body drawing now; `auto` starts `light` and may become `rich` a moment later. */
    readonly body: BodyKind;
    /** Change the mood; `animate` plays the character's entrance move and chirp, unless nobody sees the mascot then (off
     * screen, a hidden tab). False for an unknown id. */
    setMood(id: string, animate?: boolean): boolean;
    /** The mood the host asked for (the character may be showing a fallback for it). */
    getMood(): MoodId;
    /** Same as a tap; a sleeping mascot wakes up and listens. */
    tap(): void;
    setSound(on: boolean): void;
    setEffectColor(css: string): void;
    /** Measure the container again (it is watched by ResizeObserver anyway). */
    resize(): void;
    /** Stop, remove listeners, free WebGL, take the drawing out of the container. */
    dispose(): void;
}
