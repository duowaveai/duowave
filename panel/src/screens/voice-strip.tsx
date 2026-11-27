// Полоса поверх окон (docs/plans/audio-interface-3.md, «К8»): отдельное окно приложения DuoWave вверху экрана, пока
// идёт разговор с ухом приложения, — `#/voice-strip`. То же, что полоса в доке: «Слушаю» с уровнем, что услышано,
// ответ, отсчёт окна, «Жду продолжения», ×. Фокуса не берёт: печатать в своей программе она не мешает. Нажатие на текст
// — окно пульта (приложение открывает его по `POST /api/v1/talk/voice/panel`).

import { useEffect, useState } from 'preact/hooks';

import { type LiveVoice, api } from '../api';
import { tr } from '../i18n';
import { onAppVoice, useLive } from '../live';
import '../ui/voice-strip.css';

/** Что сейчас на полосе: состояние уха и его подробности. */
interface Shown {
    state: LiveVoice['state'];
    level: number;
    why?: string;
    wait?: number;
    request?: string;
    parked?: string;
}

function head(shown: Shown): string {
    switch (shown.state) {
        case 'listening':
            return tr.voice.listening;
        case 'window':
            return shown.wait ? `${tr.voice.listeningAnswer} · ${shown.wait} с` : tr.voice.listeningAnswer;
        case 'recognizing':
            return tr.voice.appRecognizing;
        case 'thinking':
            return tr.voice.thinking;
        case 'speaking':
            return tr.voice.appSpeaking;
        case 'quiet':
            return shown.parked ? tr.voice.appParked : tr.voice.appQuiet;
    }
}

export function VoiceStripPage() {
    const feed = useLive(true);
    const [shown, setShown] = useState<Shown>({ state: 'quiet', level: 0 });
    useEffect(() => {
        void api
            .talkVoice()
            .then((view) => setShown((now) => ({ ...now, ...view.state, level: 0 })))
            .catch(() => undefined);
        return onAppVoice((message) =>
            setShown({
                state: message.state,
                level: message.level ?? 0,
                why: message.why,
                wait: message.wait,
                request: message.request,
                parked: message.parked,
            }),
        );
    }, []);
    // Слова вопроса и ответа — из запроса журнала, как у полосы в доке (кому видны слова запросов).
    const request = shown.request ? feed.requests.find((one) => one.id === shown.request) : undefined;
    const caps = shown.state === 'quiet' ? shown.parked : request?.text;
    const listening = shown.state === 'listening' || shown.state === 'window';
    return (
        <div class="vs-page" role="status">
            <button type="button" class="vs-text" onClick={() => void api.voicePanel().catch(() => undefined)}>
                <span class="vs-head">{head(shown)}</span>
                {caps && <span class="vs-caps">{caps}</span>}
                {request?.answer && shown.state === 'speaking' && <span class="vs-answer">{request.answer}</span>}
            </button>
            {listening && <span class="vs-level" style={{ width: `${Math.round(shown.level * 100)}%` }} />}
            <button type="button" class="vs-close" aria-label={tr.voice.hideStrip} onClick={() => void api.voiceStop().catch(() => undefined)}>
                ×
            </button>
        </div>
    );
}
