// Клавиатура на экране для приложений (docs/plans/midi.md, «Клавиатура на экране»): от 25 до 88 клавиш, белые и
// чёрные; во всю ширину, на узком экране — прокрутка, и следующая нота всегда на виду. Следующая — цветом акцента
// темы, нажатая — затемнена, ошибка — цветом ошибки. Подписи «до», «ре»…, до первой октавы отмечено. Мышь и касание
// зовут `onDown` и `onUp` — так клавиатура на экране играет, как настоящая (пример, нет устройства).

import type { JSX } from 'preact';
import * as hooks from 'preact/hooks';

import { noteName } from './midi';

const BLACK = new Set([1, 3, 6, 8, 10]);
const isBlack = (note: number): boolean => BLACK.has(((note % 12) + 12) % 12);

export interface PianoKeysProps {
    /** Первая и последняя нота MIDI: 48–72 — 25 клавиш, 36–96 — 61, 21–108 — 88. */
    from: number;
    to: number;
    next?: number[];
    down?: number[];
    wrong?: number[];
    /** Подписывать белые клавиши и следующую ноту. */
    names?: boolean;
    onDown?: (note: number) => void;
    onUp?: (note: number) => void;
    /** Высота белой клавиши, px; 120 по умолчанию. */
    height?: number;
}

/** Клавиатура на экране. */
export function PianoKeys({ from, to, next = [], down = [], wrong = [], names = true, onDown, onUp, height = 120 }: PianoKeysProps): JSX.Element {
    // Края — белые клавиши: чёрной у края не на что опереться.
    const first = isBlack(from) ? from - 1 : from;
    const last = isBlack(to) ? to + 1 : to;
    const whites: number[] = [];
    for (let note = first; note <= last; note += 1) if (!isBlack(note)) whites.push(note);
    const width = 100 / whites.length;
    const pressed = hooks.useRef(new Map<number, number>());
    const scroller = hooks.useRef<HTMLDivElement>(null);

    // Следующая нота — на виду, если клавиатура не влезла.
    hooks.useEffect(() => {
        const target = next[0];
        const box = scroller.current;
        if (target == null || !box || box.scrollWidth <= box.clientWidth) return;
        const key = box.querySelector<HTMLElement>(`[data-note="${target}"]`);
        key?.scrollIntoView({ block: 'nearest', inline: 'center', behavior: 'smooth' });
    }, [next[0]]);

    const press = (note: number) => (event: PointerEvent) => {
        event.preventDefault();
        (event.currentTarget as HTMLElement).setPointerCapture?.(event.pointerId);
        pressed.current.set(event.pointerId, note);
        onDown?.(note);
    };
    const lift = (event: PointerEvent) => {
        const note = pressed.current.get(event.pointerId);
        if (note == null) return;
        pressed.current.delete(event.pointerId);
        onUp?.(note);
    };
    const mark = (note: number): string =>
        wrong.includes(note) ? ' is-wrong' : next.includes(note) ? ' is-next' : down.includes(note) ? ' is-down' : '';

    return (
        <div class="piano-keys" ref={scroller} role="group" aria-label="Клавиатура">
            <div class="piano-keys-bed" style={{ height: `${height}px`, minWidth: `${whites.length * 22}px` }}>
                {whites.map((note, i) => (
                    <button
                        type="button"
                        key={note}
                        data-note={note}
                        class={`piano-white${mark(note)}${note === 60 ? ' is-middle' : ''}`}
                        style={{ left: `${i * width}%`, width: `${width}%` }}
                        aria-label={noteName(note)}
                        onPointerDown={press(note)}
                        onPointerUp={lift}
                        onPointerCancel={lift}
                    >
                        {names && <span class="piano-name">{noteName(note, true)}</span>}
                    </button>
                ))}
                {whites.map((white, i) => {
                    const note = white + 1;
                    if (!isBlack(note) || note > last) return null;
                    return (
                        <button
                            type="button"
                            key={note}
                            data-note={note}
                            class={`piano-black${mark(note)}`}
                            style={{ left: `${(i + 1) * width - width * 0.3}%`, width: `${width * 0.6}%` }}
                            aria-label={noteName(note)}
                            onPointerDown={press(note)}
                            onPointerUp={lift}
                            onPointerCancel={lift}
                        >
                            {names && next.includes(note) && <span class="piano-name">{noteName(note, true)}</span>}
                        </button>
                    );
                })}
            </div>
        </div>
    );
}
