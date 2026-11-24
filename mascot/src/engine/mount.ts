// One mascot's life on a page, the same for every character: the mood the host asked for, taps and the pointer, sound,
// the frame loop (paused off screen), size changes, and which body draws. The character's rig keeps the motion; its
// bodies draw. The light body shows at once; the rich one (if the character has one and the box is big enough) takes
// its place when its chunk has loaded, and gives the place back if WebGL loses its context. The rig outlives both, so a
// swap doesn't restart the motion.

import { FALLBACK, type MoodId, type Reaction, WAKES_INTO, isMood, prefersReducedMotion } from './moods.ts';
import { playTune } from './sound.ts';
import type { Body, BodyHost, BodyKind, Character, Mascot, MascotOptions, Mood, PointerInput } from './types.ts';

/** In `auto`, a container smaller than this (either side, CSS px) keeps the light body, unless the character says otherwise. */
export const MIN_RICH_SIZE = 96;
/** In `auto`, at most this many rich bodies at once, unless the character says otherwise. */
export const MAX_RICH = 4;
/** A press that moved further than this (CSS px) is a drag, not a tap. */
const TAP_SLOP = 6;

let liveRich = 0;

/** The mood a character draws for `id`: its own look, or the nearest one along FALLBACK. */
export function drawnMood(character: Character, id: MoodId): MoodId {
    let at = id;
    while (at !== 'idle' && !character.looks[at]) at = FALLBACK[at];
    return at;
}

/** How `character` looks in `id`, after fallbacks. Hosts paint the stage behind it with this colour. */
export function lookOf(character: Character, id: MoodId): Mood {
    return { id, color: (character.looks[drawnMood(character, id)] ?? character.looks.idle).color };
}

/** Write a character with its state type checked across its rig and bodies. */
export const defineCharacter = <S>(character: Character<S>): Character<S> => character;

export function mount(container: HTMLElement, character: Character, options: MascotOptions = {}): Mascot {
    const interactive = options.interactive !== false;
    let dead = false;

    // ---------- sound ----------
    let soundOn = !!options.sound;
    let audio: AudioContext | null = null;
    const sound = (key: MoodId | Reaction): void => {
        const notes = character.tunes?.[key];
        if (soundOn && notes?.length) audio = playTune(audio, notes);
    };

    // ---------- mood and motion ----------
    let mood: MoodId = isMood(options.mood) ? options.mood : 'idle';
    const rig = character.rig({ reduced: prefersReducedMotion(), sound }, drawnMood(character, mood));
    const pointer: PointerInput = { x: 0, y: 0, drag: null };
    let state = rig.step(0, pointer);

    // ---------- bodies ----------
    if (getComputedStyle(container).position === 'static') container.style.position = 'relative';
    const touchBefore = container.style.touchAction;
    let effect = options.effectColor;
    /** The host's colour, else the page's `--mascot-effect`, else the character's own, else the text colour around it. */
    const effectCss = (): string => {
        if (effect) return effect;
        const css = getComputedStyle(container);
        return css.getPropertyValue('--mascot-effect').trim() || character.colors.effect || css.color || character.colors.ink;
    };
    const fit = (b: Body<unknown>): void => {
        const w = container.clientWidth;
        const h = container.clientHeight;
        if (w && h) b.resize(w, h);
    };
    const hostFor = (which: BodyKind): BodyHost => ({
        container,
        shadow: options.shadow !== false,
        lost: () => {
            if (which === 'rich' && kind === 'rich' && !dead) toLight();
        },
    });

    let kind: BodyKind = 'light';
    let drags = !!character.light.drag;
    let body: Body<unknown> = character.light.create(hostFor('light'));
    const dress = (b: Body<unknown>): void => {
        fit(b);
        b.setEffectColor(effectCss());
        b.draw(state, 0, pointer);
        container.style.touchAction = interactive && drags ? 'pan-y' : touchBefore;
    };
    dress(body);

    /** Put `next` in place of the current body. It is made before the old one goes, so the box is never empty. */
    const swap = (next: Body<unknown>, nextKind: BodyKind, nextDrags: boolean): void => {
        const old = body;
        body = next;
        kind = nextKind;
        drags = nextDrags;
        dress(next);
        old.dispose();
    };

    let holdsRich = false;
    const releaseRich = (): void => {
        if (holdsRich) liveRich--;
        holdsRich = false;
    };
    /** Back to the light body: WebGL lost its context. The rig keeps the mood and the motion. */
    const toLight = (): void => {
        swap(character.light.create(hostFor('light')), 'light', !!character.light.drag);
        releaseRich();
    };

    const want = options.body ?? 'auto';
    const rich = character.rich;
    /** `auto` decides once the box has a size: a box that is hidden (display: none, one of two layouts of a page)
     * mustn't hold a WebGL context it never draws in. */
    let richPending = false;
    const tryRich = (): void => {
        if (!rich || want === 'light' || dead) return;
        const rect = container.getBoundingClientRect();
        if (want === 'auto' && !(rect.width > 0 && rect.height > 0)) {
            richPending = true;
            return;
        }
        richPending = false;
        const small = Math.min(rect.width, rect.height) < (rich.minSize ?? MIN_RICH_SIZE);
        if (want === 'rich' || (!small && liveRich < (rich.maxLive ?? MAX_RICH))) {
            liveRich++;
            holdsRich = true;
            rich.load()
                .then((make) => {
                    if (dead) return;
                    let next: Body<unknown>;
                    try {
                        next = make(hostFor('rich'));
                    } catch {
                        releaseRich(); // no WebGL: stay light
                        return;
                    }
                    swap(next, 'rich', !!rich.drag);
                })
                .catch(() => releaseRich()); // the chunk did not load: stay light
        }
    };
    tryRich();

    // ---------- frames: none while off screen; requestAnimationFrame already stops in a hidden tab ----------
    let raf = 0;
    let last = 0;
    let visible = true;
    /** Frames run: the box is on screen and the page is shown. */
    const seen = (): boolean => visible && !(typeof document !== 'undefined' && document.hidden);
    const frame = (now: number): void => {
        raf = 0;
        if (dead || !visible) return;
        const dt = Math.min(0.05, (now - last) / 1000);
        last = now;
        state = rig.step(dt, pointer);
        body.draw(state, dt, pointer);
        raf = requestAnimationFrame(frame);
    };
    const start = (): void => {
        if (dead || raf || !visible) return;
        last = performance.now();
        raf = requestAnimationFrame(frame);
    };
    const io =
        typeof IntersectionObserver !== 'undefined'
            ? new IntersectionObserver((entries) => {
                  visible = entries[entries.length - 1].isIntersecting;
                  start();
              })
            : null;
    io?.observe(container);
    const refit = (): void => {
        fit(body);
        if (richPending) tryRich();
    };
    const ro = typeof ResizeObserver !== 'undefined' ? new ResizeObserver(refit) : null;
    if (ro) ro.observe(container);
    else window.addEventListener('resize', refit);

    // ---------- moods and taps ----------
    // A mood set while nobody sees the mascot (off screen, the tab in the background) comes without its move and chirp:
    // the rig gets no frames then, so every move's push would wait and all of them would land at once on return,
    // flinging the body out of its box.
    const change = (id: unknown, animate: boolean): boolean => {
        if (dead || !isMood(id)) return false;
        mood = id;
        rig.setMood(drawnMood(character, id), animate && seen());
        body.setEffectColor(effectCss());
        options.onMoodChange?.(id, lookOf(character, id));
        return true;
    };
    const tap = (): void => {
        if (dead || options.onTap?.(mood) === false) return;
        if (mood === 'sleep') {
            change(WAKES_INTO, false);
            rig.react('wake');
            return;
        }
        rig.react('tap');
    };

    // ---------- pointer: the eyes follow, a press without moving taps, a drag turns a body that turns ----------
    const listeners: Array<[string, EventListener]> = [];
    const on = <K extends keyof HTMLElementEventMap>(type: K, fn: (e: HTMLElementEventMap[K]) => void): void => {
        container.addEventListener(type, fn);
        listeners.push([type, fn as EventListener]);
    };
    if (interactive) {
        let down: { x: number; y: number; moved: boolean } | null = null;
        on('pointerdown', (e) => {
            down = { x: e.clientX, y: e.clientY, moved: false };
            try {
                container.setPointerCapture(e.pointerId);
            } catch {
                /* the pointer is already gone */
            }
        });
        on('pointermove', (e) => {
            const rect = container.getBoundingClientRect();
            pointer.x = ((e.clientX - rect.left) / rect.width) * 2 - 1;
            pointer.y = -(((e.clientY - rect.top) / rect.height) * 2 - 1);
            if (!down) return;
            const dx = e.clientX - down.x;
            const dy = e.clientY - down.y;
            if (!down.moved && Math.hypot(dx, dy) > TAP_SLOP) {
                down.moved = true;
                if (drags) container.classList.add('dragging');
            }
            if (down.moved) pointer.drag = { dx, dy };
        });
        const end = (tapToo: boolean): void => {
            if (!down) return;
            const moved = down.moved;
            down = null;
            pointer.drag = null;
            container.classList.remove('dragging');
            if (tapToo && !moved) tap();
        };
        on('pointerup', () => end(true));
        on('pointercancel', () => end(false));
        on('pointerleave', () => {
            pointer.x = 0;
            pointer.y = 0;
        });
        on('keydown', (e) => {
            if (e.key === 'Enter' || e.key === ' ') {
                e.preventDefault();
                tap();
            }
        });
    }

    const dispose = (): void => {
        if (dead) return;
        dead = true;
        cancelAnimationFrame(raf);
        io?.disconnect();
        if (ro) ro.disconnect();
        else window.removeEventListener('resize', refit);
        for (const [type, fn] of listeners) container.removeEventListener(type, fn);
        container.style.touchAction = touchBefore;
        container.classList.remove('dragging');
        body.dispose();
        releaseRich();
        void audio?.close();
    };

    start();

    return {
        character,
        get body() {
            return kind;
        },
        setMood: (id, animate = false) => change(id, animate),
        getMood: () => mood,
        tap,
        setSound: (on) => {
            soundOn = on;
            if (on) sound('idle');
        },
        setEffectColor: (css) => {
            effect = css;
            body.setEffectColor(effectCss());
        },
        resize: refit,
        dispose,
    };
}
