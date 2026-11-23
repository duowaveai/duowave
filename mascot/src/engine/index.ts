// The engine alone, without any character: for a host that brings its own (`@duowave/mascot/engine`).

export { MAX_RICH, MIN_RICH_SIZE, defineCharacter, drawnMood, lookOf, mount } from './mount.ts';
export { FALLBACK, MOOD_IDS, WAKES_INTO, isMood, prefersReducedMotion } from './moods.ts';
export type { MoodId, Reaction } from './moods.ts';
export { playTune } from './sound.ts';
export type { Tune } from './sound.ts';
export type {
    Body,
    BodyFactory,
    BodyHost,
    BodyKind,
    BodySpec,
    Character,
    Mascot,
    MascotOptions,
    Mood,
    MoodLook,
    PointerInput,
    RichBodySpec,
    Rig,
    RigContext,
    Tunes,
} from './types.ts';
