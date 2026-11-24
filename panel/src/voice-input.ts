// «Как говорить с DuoWave» (docs/plans/mic-mode.md): через колонку с Алисой или микрофоном этого компьютера. Выбор
// хранит шлюз и говорит его в `/health` (`voiceInput`); пульт держит его здесь, и всё, что зависит от режима, — что
// спрятать, как назвать, куда уходит фраза, — спрашивает одно: `onComputer()`.

import { useEffect, useState } from 'preact/hooks';

import type { VoiceInput } from './api';

let input: VoiceInput = 'station';
const listeners = new Set<(input: VoiceInput) => void>();

/** Что сказал шлюз в `/health`; шлюз постарше поля не присылает — тогда «через колонку». */
export function noteVoiceInput(next: VoiceInput | undefined): void {
    const value = next ?? 'station';
    if (value === input) return;
    input = value;
    for (const listener of [...listeners]) listener(input);
}

/** Режим «Микрофон»: говорят микрофоном этого компьютера, Алисы, Яндекса и колонки в пульте нет. */
export function onComputer(): boolean {
    return input === 'computer';
}

/** Чей голос говорит — `/health` `tts` (VoiceService::describe): «own…», «mixed…», «yandex…», «off». */
let tts = '';

export function noteVoice(next: string | undefined): void {
    tts = next ?? '';
}

/**
 * Скажет ли компьютер ответ вслух: голос дома свой — модель на компьютере или облачная. Голос Алисы звучит только через
 * колонку (`/api/v1/talk/speech` ответил бы 409): тогда ответ — текстом (решение владельца к находке 9 проверки шага 11).
 */
export function computerVoice(): boolean {
    return /^(own|mixed)\b/.test(tts) && !tts.includes('TTS_OWN_URL not set');
}

/** То же для отрисовки: перерисовывает, когда режим меняется. */
export function useOnComputer(): boolean {
    const [value, setValue] = useState(input);
    useEffect(() => {
        setValue(input);
        listeners.add(setValue);
        return () => {
            listeners.delete(setValue);
        };
    }, []);
    return value === 'computer';
}
