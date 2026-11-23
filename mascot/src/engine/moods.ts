// The mood vocabulary: what the assistant is doing, in ids every character understands. Hosts speak only these ids and
// keep their own words (labels, lines); each character decides how a mood looks. A character must have a look for
// `idle`; for a mood it has no look for, it shows the nearest one along FALLBACK, so a new character can start small.

/** Every mood, in the order a demo shows them. */
export const MOOD_IDS = ['idle', 'tender', 'wink', 'sleep', 'listen', 'think', 'speak', 'success', 'surprise', 'sad'] as const;

export type MoodId = (typeof MOOD_IDS)[number];

/** Moments that are not moods: a tap, and waking up from `sleep` by a tap. */
export type Reaction = 'tap' | 'wake';

/** What a character shows for a mood it has no look for. Every chain ends at `idle`. */
export const FALLBACK: Readonly<Record<Exclude<MoodId, 'idle'>, MoodId>> = {
    tender: 'success',
    wink: 'success',
    success: 'idle',
    sleep: 'idle',
    listen: 'idle',
    think: 'idle',
    speak: 'idle',
    surprise: 'idle',
    sad: 'idle',
};

/** A tap wakes a sleeping mascot into this mood: asleep means «until called». The same for every character. */
export const WAKES_INTO: MoodId = 'listen';

export const isMood = (id: unknown): id is MoodId => typeof id === 'string' && (MOOD_IDS as readonly string[]).includes(id);

export function prefersReducedMotion(): boolean {
    try {
        return matchMedia('(prefers-reduced-motion: reduce)').matches;
    } catch {
        return false;
    }
}
