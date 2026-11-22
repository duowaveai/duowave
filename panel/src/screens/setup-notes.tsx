// Строки «Ещё не всё готово» (замечание 17 после 0.18.1): что ставится с ходом, когда заработает слово, где голос
// персонажа, кто отвечает без мышления. Данные — screens/setup-truth.ts.

import { tr } from '../i18n';
import { plainName } from '../voice/pending-voice';
import type { SetupTruth } from './setup-truth';

/** Строки того, что не готово; пусто — null. `thinks` — мышление выбрано (у витрины — её `answering`). */
export function setupLines(truth: SetupTruth, wake: string, thinks: boolean | undefined): string[] {
    const t = tr.onbTry;
    const lines: string[] = [];
    if (truth.installing) {
        const { title, done, total, detail } = truth.installing;
        lines.push(`${t.setupInstalling(tr.wizard.quote(plainName(title)), done, total)}${detail ? ` ${detail}.` : ''} ${t.setupWakeMeanwhile(wake)}`);
    }
    // Слово слышно только в «Алиса + ваш ИИ»: в «Алисе» (мышления нет — режим не переключали) его не слышит никто.
    if (truth.word && !truth.installing && truth.mode === 'relay') lines.push(`${t.setupWordWaits(truth.word)} ${t.setupWakeMeanwhile(wake)}`);
    if (truth.character && thinks === false) lines.push(t.setupNoThinking(truth.character));
    if (truth.character && truth.voiceAway) lines.push(t.setupVoiceAway(truth.character));
    return lines;
}

/** Слово, которым звать сейчас: слово приложения, когда оно уже встало и его слышно, иначе прежнее. */
export function wakeNow(truth: SetupTruth, wake: string): string {
    return truth.word && !truth.installing && truth.mode !== 'relay' ? truth.word : wake;
}
