// Полоса голоса над доком (docs/plans/voice-creation.md, «Голос пульта»; макет docs/design/voice-creation, шаги «Сказали»,
// «Готово», «На столе»): кто сейчас говорит и что сказано. Пульт слушает — уровень, потом слова крупно и где их
// слышали (облако — янтарём); агент взял работу — его слова и где он (облако — янтарь, дома — своё); готово — строка
// шлюза, отсчёт слушания и подсказки «Можно сказать»; поле ключа открыто — микрофон выключен. Подсказки — кнопки:
// что на них, то и говорят (phrases.ts).

import { useEffect, useState } from 'preact/hooks';

import { IconMic, IconMicOff } from '../icons';
import { tr } from '../i18n';

/** Чей цвет у метки: своё и дома — лайм, снаружи — янтарь, агент — фиолет. */
export type Tone = 'own' | 'out' | 'agent';

export interface StripChip {
    label: string;
    /** Что делает подсказка; без него — слова для голоса, нажатием не делается ничего. */
    act?: () => void;
}

export interface StripMessage {
    /**
     * listen — пульт слушает; agent — агент взял работу; ready — готово, ждём ответа; done — сделано; hold — микрофон
     * выключен; note — просто слова; answer — ответ «Мозга» в режиме «Микрофон»: вопрос крупно, ответ растёт по фразам.
     */
    kind: 'listen' | 'agent' | 'ready' | 'done' | 'hold' | 'note' | 'answer';
    head: string;
    /** Метка справа: где слышали, где агент, сколько ещё слушаю. */
    chip?: { text: string; tone: Tone };
    /** Слова крупно — что сказал человек. */
    caps?: string;
    /** Строка — что отвечают. */
    text?: string;
    chips?: StripChip[];
    /** Мелкая строка под подсказками. */
    after?: string;
    /** Уровень микрофона 0…1, пока слушаем. */
    level?: number;
    /** Ждём ответа столько секунд (кольцо у микрофона). */
    wait?: number;
}

type Listener = (message: StripMessage | null) => void;

// «Текстом колонке…» (решение 1): прежнее «Сказать через колонку» — лист с текстом, который колонка произнесёт; его
// открывает пульт (app.tsx), а просят о нём поиск и полоса.
const sayListeners = new Set<() => void>();

/** Открыть «Текстом колонке…». */
export function requestTextToStation(): void {
    for (const listener of [...sayListeners]) listener();
}

/** Кто открывает «Текстом колонке…» (пульт). Возвращает, как перестать. */
export function onTextToStation(listener: () => void): () => void {
    sayListeners.add(listener);
    return () => sayListeners.delete(listener);
}

let current: StripMessage | null = null;
let hideAt = 0;
let timer = 0;
const listeners = new Set<Listener>();

function tell(): void {
    for (const listener of [...listeners]) listener(current);
}

/** Показать полосу; `ms` — сколько держать (0 — пока не сменят). */
export function showStrip(message: StripMessage, ms = 0): void {
    window.clearTimeout(timer);
    current = message;
    hideAt = ms ? Date.now() + ms : 0;
    if (ms) timer = window.setTimeout(() => hideStrip(), ms);
    tell();
}

/**
 * Поправить то, что на полосе сейчас (уровень микрофона, отсчёт, новая фраза ответа), если на ней это же. `ms` —
 * сколько ещё держать с этой минуты; без него срок прежний.
 */
export function updateStrip(kind: StripMessage['kind'], patch: Partial<StripMessage>, ms?: number): void {
    if (!current || current.kind !== kind) return;
    current = { ...current, ...patch };
    if (ms !== undefined) {
        window.clearTimeout(timer);
        hideAt = ms ? Date.now() + ms : 0;
        if (ms) timer = window.setTimeout(() => hideStrip(), ms);
    }
    tell();
}

// «×» на полосе — рука человека: кто говорит ответ (voice/speak.ts), обрывает и голос. Полоса, ушедшая по времени,
// никого не обрывает.
const closeListeners = new Set<() => void>();

/** Кому сказать, что полосу закрыли рукой. Возвращает, как перестать. */
export function onStripClosed(listener: () => void): () => void {
    closeListeners.add(listener);
    return () => closeListeners.delete(listener);
}

/** «×»: закрыть полосу и оборвать то, что за ней стоит. */
export function closeStrip(): void {
    for (const listener of [...closeListeners]) listener();
    hideStrip();
}

export function hideStrip(): void {
    window.clearTimeout(timer);
    current = null;
    hideAt = 0;
    tell();
}

/** Что на полосе сейчас. */
export function stripNow(): StripMessage | null {
    return hideAt && Date.now() > hideAt ? null : current;
}

export function useStrip(): StripMessage | null {
    const [message, setMessage] = useState<StripMessage | null>(stripNow);
    useEffect(() => {
        listeners.add(setMessage);
        return () => {
            listeners.delete(setMessage);
        };
    }, []);
    return message;
}

/** Микрофон полосы: слушает — белый с зелёным кольцом, ждёт ответа — кольцо отсчёта, выключен — серый перечёркнутый. */
function StripMic({ message }: { message: StripMessage }) {
    if (message.kind === 'hold')
        return (
            <span class="strip-mic is-off" aria-hidden="true">
                <IconMicOff size={18} />
            </span>
        );
    if (message.kind !== 'listen' && !message.wait) return null;
    const level = Math.max(0, Math.min(1, message.level ?? 0));
    return (
        <span class={`strip-mic${message.kind === 'listen' ? ' is-listening' : ''}${message.wait ? ' is-waiting' : ''}`} aria-hidden="true">
            {message.wait ? (
                <svg class="strip-mic-ring" viewBox="0 0 44 44">
                    <circle cx="22" cy="22" r="20" style={{ animationDuration: `${message.wait}s` }} />
                </svg>
            ) : null}
            <span class="strip-mic-level" style={{ transform: `scale(${1 + level * 0.35})` }} />
            <IconMic size={18} />
        </span>
    );
}

/** Полоса над доком. Пусто — её нет. */
export function VoiceStrip() {
    const message = useStrip();
    if (!message) return null;
    const dot = message.kind === 'agent' || message.kind === 'ready' ? 'agent' : message.kind === 'done' || message.kind === 'answer' ? 'own' : null;
    return (
        <section class={`voice-strip is-${message.kind}`} aria-live="polite">
            <div class="voice-strip-head">
                <StripMic message={message} />
                {dot && <span class={`voice-strip-dot is-${dot}`} aria-hidden="true" />}
                <span class="voice-strip-who">{message.head}</span>
                {message.chip && (
                    <span class={`voice-strip-chip is-${message.chip.tone}`}>
                        <span class="voice-strip-chip-dot" aria-hidden="true" />
                        {message.chip.text}
                    </span>
                )}
                <button class="voice-strip-close" aria-label={tr.voice.hideStrip} onClick={closeStrip}>
                    ×
                </button>
            </div>
            {message.caps && <p class="voice-strip-caps">{message.caps}</p>}
            {message.text && <p class="voice-strip-text">{message.text}</p>}
            {message.chips && message.chips.length > 0 && (
                <div class="voice-strip-chips">
                    <span class="w-cap">{tr.voice.canSay}</span>
                    {message.chips.map((chip) => (
                        <button key={chip.label} class="say-chip" onClick={chip.act} disabled={!chip.act}>
                            {chip.label}
                        </button>
                    ))}
                </div>
            )}
            {message.after && <p class="w-cap m-0">{message.after}</p>}
        </section>
    );
}
