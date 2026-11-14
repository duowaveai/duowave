// Два звука микрофона (docs/plans/audio-interface-2.md, «К4. Сигналы»): «слушаю» — микрофон открылся, по кнопке и сам
// после нашего вопроса; «услышала» — мысль кончилась и пошла в ответ. Оба — тона, которые пульт рисует сам
// (WebAudio), без файлов и без записей голоса; от языка не зависят. Всё остальное говорится словами. Выключатель —
// «Настройки» → «Микрофон» → «Звуки микрофона»; хранится в этом браузере, по умолчанию включено.

import { routeContext } from './devices';

/** Сколько длится «слушаю»: пока он звучит, пульт не считает звук речью (микрофон его слышит). */
export const LISTEN_TONE_MS = 220;

const KEY = 'duowave.micSounds';

type Tone = 'listen' | 'heard';

/** Ноты звука: частота (Гц), начало и длина (мс). */
const NOTES: Record<Tone, Array<{ hz: number; at: number; ms: number }>> = {
    // Два коротких восходящих тона.
    listen: [
        { hz: 660, at: 0, ms: 80 },
        { hz: 880, at: 95, ms: 100 },
    ],
    // Один мягкий тон ниже.
    heard: [{ hz: 520, at: 0, ms: 110 }],
};

let context: AudioContext | null = null;

/** Звучат ли звуки микрофона. */
export function micSounds(): boolean {
    try {
        return localStorage.getItem(KEY) !== 'off';
    } catch {
        return true;
    }
}

/** Выход звука сменили: звуки — туда же. */
export function rerouteTones(): void {
    if (context) routeContext(context);
}

/** Включить или выключить звуки микрофона. */
export function setMicSounds(on: boolean): void {
    try {
        localStorage.setItem(KEY, on ? 'on' : 'off');
    } catch {
        // Хранилище закрыто — звуки как по умолчанию.
    }
}

/** Сыграть звук, если звуки включены. Не вышло (браузер не дал звук) — молча. */
export function playTone(tone: Tone): void {
    if (!micSounds()) return;
    try {
        if (!context) {
            context = new AudioContext();
            routeContext(context);
        }
        void context.resume();
        const start = context.currentTime + 0.01;
        for (const note of NOTES[tone]) {
            const osc = context.createOscillator();
            const gain = context.createGain();
            osc.type = 'sine';
            osc.frequency.value = note.hz;
            const from = start + note.at / 1000;
            const to = from + note.ms / 1000;
            // Мягкие края: без щелчка.
            gain.gain.setValueAtTime(0, from);
            gain.gain.linearRampToValueAtTime(0.08, from + 0.015);
            gain.gain.setValueAtTime(0.08, to - 0.03);
            gain.gain.linearRampToValueAtTime(0, to);
            osc.connect(gain).connect(context.destination);
            osc.start(from);
            osc.stop(to + 0.01);
        }
    } catch {
        // Звука нет — микрофон работает и без него.
    }
}
