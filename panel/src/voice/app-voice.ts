// Ухо и голос приложения этого компьютера (docs/plans/audio-interface-3.md, «К5»). Если ухо есть и этот пульт —
// на этом компьютере (`GET /api/v1/talk/voice` → `here`), пульт сам не пишет и не играет: «Сказать» и клавиша зовут
// `press`, полоса рисуется по сообщениям `voice` живого канала, «Готово» пульт не говорит — только показывает.
// Иначе — второй этап как есть (браузер на телефоне, шлюз без приложения).

import { useEffect, useState } from 'preact/hooks';

import { type LiveVoice, type TalkVoiceView, api } from '../api';
import { tr } from '../i18n';
import { hideStrip, showStrip, stripNow, updateStrip } from './strip';
import type { TalkKey } from './talk-key';

let view: TalkVoiceView | null = null;
const listeners = new Set<() => void>();

function tell(): void {
    for (const listener of [...listeners]) listener();
}

/** Ухо приложения есть, и пульт — на этом компьютере. */
export function appVoiceHere(): boolean {
    return !!view?.port && !!view.here;
}

/** Что известно об ухе приложения. */
export function appVoice(): TalkVoiceView | null {
    return view;
}

/** Перечитать: после входа, на новом канале и после смены настроек. */
export async function readAppVoice(): Promise<TalkVoiceView | null> {
    try {
        view = await api.talkVoice();
    } catch {
        view = null;
    }
    tell();
    return view;
}

/** То же, для экрана: перерисовка при перемене. */
export function useAppVoice(): TalkVoiceView | null {
    const [now, setNow] = useState(view);
    useEffect(() => {
        const listener = () => setNow(view);
        listeners.add(listener);
        if (!view) void readAppVoice();
        return () => void listeners.delete(listener);
    }, []);
    return now;
}

/** «Сказать» и клавиша при ухе приложения: компьютер слушает сам. */
export function pressAppVoice(): void {
    void api.voicePress().catch(() => undefined);
}

/** Отпустили клавишу после удержания: мысль кончилась сейчас. */
export function releaseAppVoice(heldMs: number): void {
    void api.voiceRelease(heldMs).catch(() => undefined);
}

/** Полоса по сообщению уха: «Слушаю» с уровнем, «Распознаю», «Думаю», окно с отсчётом, «Жду продолжения». */
export function heardAppVoice(message: LiveVoice): void {
    if (view) view = { ...view, state: { ...view.state, state: message.state, why: message.why, wait: message.wait, parked: message.parked } };
    tell();
    if (!appVoiceHere()) return;
    const chip = { text: tr.voice.heardHere, tone: 'own' as const };
    switch (message.state) {
        case 'listening':
        case 'window': {
            const head = message.state === 'window' ? tr.voice.listeningAnswer : tr.voice.listening;
            const shown = stripNow();
            if (shown?.kind === 'listen') updateStrip('listen', { head, level: message.level ?? shown.level, wait: message.wait });
            else showStrip({ kind: 'listen', head, chip, level: message.level ?? 0, wait: message.wait, caps: message.parked });
            return;
        }
        case 'recognizing':
            showStrip({ kind: 'listen', head: tr.voice.appRecognizing, chip });
            return;
        case 'thinking':
            showStrip({ kind: 'note', head: tr.voice.thinking, chip });
            return;
        case 'speaking':
            // 27.11: «Думаю…» stayed here under the answer while the strip over the windows said «Отвечаю».
            showStrip({ kind: 'note', head: tr.voice.appSpeaking, chip });
            return;
        case 'quiet': {
            if (message.parked) {
                showStrip({ kind: 'note', head: tr.voice.appParked, text: message.parked }, 6000);
                return;
            }
            // The ear's own strip goes with the talk: «Слушаю», «Думаю…», «Отвечаю» — not someone else's message.
            const shown = stripNow();
            if (shown && (shown.kind === 'listen' || shown.chip?.text === chip.text)) hideStrip();
            return;
        }
    }
}

/** Клавиша пульта как её называет шлюз (`Alt+Space`); не каждая годится для всех программ — тогда null. */
export function keyName(key: TalkKey): string | null {
    const code = key.code;
    const name = code === 'Space'
        ? 'Space'
        : /^Key[A-Z]$/.test(code)
          ? code.slice(3)
          : /^Digit[0-9]$/.test(code)
            ? code.slice(5)
            : /^F[0-9]{1,2}$/.test(code)
              ? code
              : null;
    if (!name) return null;
    const parts = [key.ctrl && 'Control', key.alt && 'Alt', key.shift && 'Shift', key.meta && 'Command'].filter(Boolean) as string[];
    if (!parts.length) return null;
    return [...parts, name].join('+');
}
