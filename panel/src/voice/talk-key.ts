// Клавиша «Сказать» (docs/plans/mic-mode.md, решение 11): в окне пульта — то же, что кнопка. По умолчанию ⌥Пробел;
// он бывает занят другой программой (Raycast, переключатель раскладки), и тогда клавишу выбирают в «Настройках» →
// «Микрофон» — нажатием. Помнится в этом браузере: клавиатура у каждого компьютера своя, права тут ни при чём.

import { useEffect, useState } from 'preact/hooks';

import { tr } from '../i18n';

export interface TalkKey {
    /** `KeyboardEvent.code`: место клавиши, а не буква — ⌥ на Mac меняет букву (⌥A — «å»). */
    code: string;
    alt: boolean;
    ctrl: boolean;
    meta: boolean;
    shift: boolean;
}

const STORE = 'panel-talk-key';

export const DEFAULT_TALK_KEY: TalkKey = { code: 'Space', alt: true, ctrl: false, meta: false, shift: false };

/** Клавиши, которые сами по себе — не клавиша «Сказать»: модификаторы и то, чем правят текст и уходят из окна. */
const NEVER = new Set([
    'AltLeft',
    'AltRight',
    'ControlLeft',
    'ControlRight',
    'MetaLeft',
    'MetaRight',
    'ShiftLeft',
    'ShiftRight',
    'CapsLock',
    'Escape',
    'Tab',
    'Enter',
    'NumpadEnter',
    'Backspace',
    'Delete',
    'Fn',
]);

function valid(key: unknown): key is TalkKey {
    if (!key || typeof key !== 'object') return false;
    const one = key as Record<string, unknown>;
    return typeof one.code === 'string' && ['alt', 'ctrl', 'meta', 'shift'].every((name) => typeof one[name] === 'boolean');
}

let current: TalkKey = (() => {
    try {
        const stored = JSON.parse(localStorage.getItem(STORE) ?? 'null');
        return valid(stored) ? stored : DEFAULT_TALK_KEY;
    } catch {
        return DEFAULT_TALK_KEY;
    }
})();

const listeners = new Set<(key: TalkKey) => void>();

export function talkKey(): TalkKey {
    return current;
}

export function setTalkKey(next: TalkKey): void {
    current = next;
    try {
        if (same(next, DEFAULT_TALK_KEY)) localStorage.removeItem(STORE);
        else localStorage.setItem(STORE, JSON.stringify(next));
    } catch {
        // приватное окно: помним до перезагрузки
    }
    for (const listener of [...listeners]) listener(next);
}

/** Клавиша «Сказать» для отрисовки: перерисовывает, когда её поменяли. */
export function useTalkKeyChoice(): TalkKey {
    const [value, setValue] = useState(current);
    useEffect(() => {
        setValue(current);
        listeners.add(setValue);
        return () => {
            listeners.delete(setValue);
        };
    }, []);
    return value;
}

export function same(a: TalkKey, b: TalkKey): boolean {
    return a.code === b.code && a.alt === b.alt && a.ctrl === b.ctrl && a.meta === b.meta && a.shift === b.shift;
}

/** Пока клавишу выбирают в «Настройках», прежняя не слушает: нажатие — это выбор, а не «Сказать». */
let choosing = false;

export function choosingTalkKey(): boolean {
    return choosing;
}

export function setChoosingTalkKey(on: boolean): void {
    choosing = on;
}

/** Нажата ли клавиша «Сказать». */
export function isTalkKey(event: KeyboardEvent, key: TalkKey = current): boolean {
    return (
        !event.repeat &&
        event.code === key.code &&
        event.altKey === key.alt &&
        event.ctrlKey === key.ctrl &&
        event.metaKey === key.meta &&
        event.shiftKey === key.shift
    );
}

/** F1…F24: им модификатор не нужен — текст ими не набирают. */
const functionKey = (code: string): boolean => /^F([1-9]|1\d|2[0-4])$/.test(code);

/**
 * Нажатие как клавиша «Сказать» — или почему не годится. Без модификатора годятся только F1…F24: иначе клавиша
 * съела бы букву или пробел в поле ввода. ⌘K и Ctrl+K — поиск пульта.
 */
export function keyOf(event: KeyboardEvent): TalkKey | { problem: string } | null {
    if (NEVER.has(event.code) || !event.code) return null;
    const key: TalkKey = { code: event.code, alt: event.altKey, ctrl: event.ctrlKey, meta: event.metaKey, shift: event.shiftKey };
    const modified = key.alt || key.ctrl || key.meta;
    if (!modified && !functionKey(key.code)) return { problem: tr.voiceInput.keyNeedsModifier };
    if ((key.meta || key.ctrl) && !key.alt && !key.shift && key.code === 'KeyK') return { problem: tr.voiceInput.keyIsSearch };
    return key;
}

/** Название клавиши, как на Mac: «⌥Пробел», «⌃⇧D», «F5». */
export function keyLabel(key: TalkKey): string {
    const marks = `${key.ctrl ? '⌃' : ''}${key.alt ? '⌥' : ''}${key.shift ? '⇧' : ''}${key.meta ? '⌘' : ''}`;
    return marks + codeName(key.code);
}

function codeName(code: string): string {
    if (code === 'Space') return tr.voiceInput.keySpace;
    if (code.startsWith('Key')) return code.slice(3);
    if (code.startsWith('Digit')) return code.slice(5);
    if (code.startsWith('Numpad')) return `Num ${code.slice(6)}`;
    const arrows: Record<string, string> = { ArrowUp: '↑', ArrowDown: '↓', ArrowLeft: '←', ArrowRight: '→' };
    const signs: Record<string, string> = {
        Backquote: '`',
        Minus: '-',
        Equal: '=',
        BracketLeft: '[',
        BracketRight: ']',
        Backslash: '\\',
        Semicolon: ';',
        Quote: "'",
        Comma: ',',
        Period: '.',
        Slash: '/',
        IntlBackslash: '§',
    };
    return arrows[code] ?? signs[code] ?? code;
}
