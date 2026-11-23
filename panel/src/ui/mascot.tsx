// The mascot as a Preact component: the mascot library (mascot/README.md) in a box the size the host gives it. Whoever
// DuoWave shows now (the library's CURRENT character, Deevi today) unless a `character` is named. The light body shows at
// once; a box big enough gets the rich body a moment later (Deevi's 3D, its own chunk, loaded only then). The mood
// follows the prop. No words inside the library: the name a screen reader says comes from the panel's catalog, by id.

import { useEffect, useRef } from 'preact/hooks';

import { type Character, CURRENT, type Mascot as Handle, type MascotOptions, type MoodId, createMascot } from '@duowave/mascot';

import { tr } from '../i18n';

/** Each character's name in the panel's languages. A new character adds its line here. */
const NAMES: Record<string, () => string> = {
    deevi: () => tr.stations.divi,
};

export const mascotName = (character: Character): string => NAMES[character.id]?.() ?? character.id;

export interface MascotProps {
    mood?: MoodId;
    /** Who to draw; the library's CURRENT character by default. */
    character?: Character;
    /** `auto` (default): the rich body when the box is big enough; `light` never loads it; `rich` tries it whatever the size. */
    body?: MascotOptions['body'];
    /** Tap to react, eyes follow the pointer, Enter and Space tap. Default true. */
    interactive?: boolean;
    /** What a screen reader hears: the character's name by default; `false` where the mascot is decoration. */
    label?: string | false;
    /** Return false to skip the character's reaction. */
    onTap?: (mood: MoodId) => boolean | void;
    /** Enter with the mood's own move (a hop for `success`), not standing in it already. */
    enter?: boolean;
    /** Soft ground shadow; off where the host draws its own. Default true. */
    shadow?: boolean;
    class?: string;
    style?: string | Record<string, string | number>;
}

export function Mascot({
    mood = 'idle',
    character = CURRENT,
    body = 'auto',
    interactive = true,
    label,
    onTap,
    enter,
    shadow,
    class: className,
    style,
}: MascotProps) {
    const host = useRef<HTMLDivElement>(null);
    const handle = useRef<Handle | null>(null);
    const shown = useRef<MoodId>(mood);
    const tapRef = useRef(onTap);
    tapRef.current = onTap;

    useEffect(() => {
        if (!host.current) return;
        const made = createMascot(host.current, {
            character,
            mood: enter ? 'idle' : shown.current,
            body,
            interactive,
            shadow,
            onTap: (id) => tapRef.current?.(id),
        });
        if (enter) made.setMood(shown.current, true);
        handle.current = made;
        return () => {
            made.dispose();
            handle.current = null;
        };
        // `enter` and `shadow` matter only when it first appears.
    }, [character, body, interactive]);

    useEffect(() => {
        if (mood === shown.current) return;
        shown.current = mood;
        handle.current?.setMood(mood, true);
    }, [mood]);

    const said = label === false ? undefined : (label ?? mascotName(character));
    return (
        <div
            ref={host}
            class={className}
            style={style}
            role={said ? 'img' : undefined}
            aria-label={said}
            aria-hidden={said ? undefined : 'true'}
            tabIndex={interactive && said ? 0 : undefined}
        />
    );
}
