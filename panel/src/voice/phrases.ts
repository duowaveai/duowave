// Фразы у кнопок (docs/plans/voice-creation.md, принцип 2: «что на кнопках — то и говорите»): видимая кнопка называет
// свои фразы, пока она на экране, — подпись и есть фраза, скрытых команд нет. Услышанное пульт сначала сверяет с фразами
// на экране; не нашлось — это просьба «сделай…» или «Не понятно». Одобрение («Одобрить и поставить») и поле ключа
// фраз не называют: «да» — рукой, личное — только руками. Пока поле личного открыто, микрофон выключен (`hold`).

import { useEffect, useRef, useState } from 'preact/hooks';

interface Action {
    words: string[];
    run: (rest: string, phrase: string) => void;
    at: number;
    /** Подсказывать ли фразу в «Можно сказать». */
    hint: boolean;
}

let next = 0;
const actions = new Map<number, Action>();

/** Слова как их сверяют: строчные, «ё» как «е», без знаков и без «Алиса» в начале. */
export function plain(text: string): string {
    return text
        .toLowerCase()
        .replace(/ё/g, 'е')
        .replace(/[^\p{L}\p{N}\s-]/gu, ' ')
        .replace(/\s+/g, ' ')
        .trim()
        .replace(/^(алиса|яндекс)\s+/, '');
}

/**
 * Кнопка называет свои фразы, пока она на экране: услышали фразу (или фразу и слова после неё — `rest`) — `run`, с той
 * фразой, что узнали. Фраза с «~» в начале — основа слова: «~показыв» узнаёт «показывай три» и «показывать 3».
 * Пустой список — кнопка голосу недоступна.
 */
export function useVoiceAction(words: string[], run: (rest: string, phrase: string) => void, hint = true): void {
    const latest = useRef(run);
    latest.current = run;
    const key = words.join('|');
    useEffect(() => {
        if (!words.length) return;
        const id = (next += 1);
        actions.set(id, {
            words: words.map((word) => (word.startsWith('~') ? `~${plain(word.slice(1))}` : plain(word))),
            run: (rest, phrase) => latest.current(rest, phrase),
            at: id,
            hint,
        });
        return () => {
            actions.delete(id);
        };
    }, [key]);
}

/**
 * Что из видимого на экране называет услышанное: точная фраза раньше начала фразы, длинная — раньше короткой, новее
 * поставленная кнопка — раньше старой (окно поверх стола).
 */
export function matchAction(heard: string): { run: () => void; phrase: string } | null {
    const said = plain(heard);
    if (!said) return null;
    let best: { action: Action; phrase: string; exact: boolean } | null = null;
    for (const action of actions.values()) {
        for (const word of action.words) {
            // Основа слова: «~показыв» — первое слово сказанного начинается с неё.
            const stem = word.startsWith('~') ? word.slice(1) : null;
            const first = said.split(' ')[0];
            const phrase = stem && first.startsWith(stem) ? first : stem ? '' : word;
            if (!phrase) continue;
            const exact = said === phrase;
            if (!exact && !said.startsWith(`${phrase} `)) continue;
            const better =
                !best ||
                (exact && !best.exact) ||
                (exact === best.exact && phrase.length > best.phrase.length) ||
                (exact === best.exact && phrase.length === best.phrase.length && action.at > best.action.at);
            if (better) best = { action, phrase, exact };
        }
    }
    if (!best) return null;
    const { action, phrase } = best;
    const rest = said.slice(phrase.length).trim();
    return { run: () => action.run(rest, phrase), phrase };
}

/** Фразы, что видны на экране сейчас: подсказки «Можно сказать». */
export function visiblePhrases(): string[] {
    const seen = new Set<string>();
    for (const action of [...actions.values()].reverse()) if (action.hint && action.words[0] && !action.words[0].startsWith('~')) seen.add(action.words[0]);
    return [...seen];
}

// Микрофон выключен, пока открыто поле личного: держат его поля, каждое своё.
const holds = new Set<number>();
const holdListeners = new Set<(held: boolean) => void>();

/** Микрофон выключен сейчас: открыто поле личного. */
export function micHeld(): boolean {
    return holds.size > 0;
}

function tellHold(): void {
    for (const listener of [...holdListeners]) listener(micHeld());
}

/** Поле личного открыто (`on`): пока так, пульт не слушает, и это видно в окне и в доке. */
export function useMicHold(on: boolean): void {
    useEffect(() => {
        if (!on) return;
        const id = (next += 1);
        holds.add(id);
        tellHold();
        return () => {
            holds.delete(id);
            tellHold();
        };
    }, [on]);
}

/** Выключен ли микрофон сейчас — для дока и полосы. */
export function useMicHeld(): boolean {
    const [held, setHeld] = useState(micHeld);
    useEffect(() => {
        holdListeners.add(setHeld);
        return () => {
            holdListeners.delete(setHeld);
        };
    }, []);
    return held;
}
