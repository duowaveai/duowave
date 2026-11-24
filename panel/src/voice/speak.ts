// Пульт говорит (docs/plans/mic-mode.md, «Пульт говорит»): фраза, которая не кнопка на экране, уходит в разговор
// (`POST /api/v1/talk`; docs/plans/audio-interface.md, решение 6) — сначала к правилам слоя (работы, дверь, ответ на
// вопрос), потом «Мозгу». Ответ приходит по фразам — на полосу и голосом компьютера: в режиме «Микрофон» всегда, в
// режиме «Колонка» — когда голос дома свой (голос Алисы звучит только через колонку, тогда пульт пишет текстом).
// Голос каждой фразы просится у шлюза, как только фраза пришла, но не дальше одной вперёд: сервер своего голоса
// говорит по одной. Звучат они по порядку, следующая — по концу предыдущей. Новое «Сказать», «×» на полосе или
// ошибка обрывают всё: и ответ «Мозга», и голос. Кончился ответ вопросом — зовётся `after`: микрофон слушает сам.

import { TalkEvent, TalkTurn, WindowWhy, problemOf, talk, talkSpeech } from '../api';
import { startedHere } from './works';
import { playToEnd, stopAudio } from '../audio';
import { tr } from '../i18n';
import { hideStrip, onStripClosed, showStrip, updateStrip } from './strip';

/** После вопроса микрофон слушает столько без нажатия. */
export const ANSWER_WAIT_MS = 6_000;
/** Сколько полоса держит ответ после последнего слова. */
const KEEP_MS = 12_000;
/** Ключ звука ответа в общем проигрывателе пульта: звучит одно за раз. */
const AUDIO_KEY = 'talk-answer';

interface Phrase {
    text: string;
    /** Голос фразы: просится, когда до неё осталась одна. */
    voice?: Promise<Blob | null>;
}

interface Answer {
    abort: AbortController;
    phrases: Phrase[];
    /** Сколько фраз уже сказано (или пропущено без голоса). */
    spoken: number;
    /** «Мозг» дописал ответ. */
    written: boolean;
    /** Голоса нет (не выбран, не запустился) или его остановили: ответ остаётся текстом. */
    mute: boolean;
    speaking: boolean;
    /** Микрофон открывается сам после ответа — и почему (docs/plans/audio-interface-2.md, «К2»). */
    after?: (why: WindowWhy) => void;
    /** Чем кончился ответ: спрашивает ли, напоминание, не принят ли. */
    done?: Extract<TalkEvent, { type: 'done' }>;
}

let current: Answer | null = null;
const listeners = new Set<(on: boolean) => void>();

function tell(): void {
    for (const listener of [...listeners]) listener(!!current);
}

/** Отвечает ли пульт сейчас (пишется ответ или звучит голос). */
export function answering(): boolean {
    return !!current;
}

/** Слушать, отвечает ли пульт. */
export function onAnswering(listener: (on: boolean) => void): () => void {
    listeners.add(listener);
    return () => listeners.delete(listener);
}

/** Оборвать ответ: и «Мозг», и голос. Полосу не трогает. */
export function stopAnswer(): void {
    const answer = current;
    if (!answer) return;
    current = null;
    answer.abort.abort();
    stopAudio();
    tell();
}

// «×» на полосе обрывает и голос.
onStripClosed(stopAnswer);

const chipOf = (by: 'home' | 'cloud') => (by === 'cloud' ? { text: tr.voice.chipCloud, tone: 'out' as const } : { text: tr.voice.chipHome, tone: 'own' as const });

/** Кончается ли ответ вопросом: после него микрофон слушает сам. */
export function endsWithQuestion(text: string): boolean {
    return /\?[\s»")\]]*$/.test(text.trim());
}

/** Голос фразы — у той, что звучит, и у следующей за ней: одна вперёд. */
function ask(answer: Answer): void {
    if (answer.mute) return;
    for (const phrase of answer.phrases.slice(answer.spoken, answer.spoken + 2)) {
        phrase.voice ??= talkSpeech(phrase.text, answer.abort.signal).catch((error) => {
            // Голоса нет — ответ остаётся на полосе текстом, и сказано почему. Оборвали — молча.
            if (current === answer && !answer.abort.signal.aborted && !answer.mute) {
                answer.mute = true;
                updateStrip('answer', { after: problemOf(error) });
            }
            return null;
        });
    }
}

/** Говорить фразы по порядку, пока они есть; кончились и ответ дописан — закончить. */
async function speak(answer: Answer): Promise<void> {
    if (answer.speaking) return;
    answer.speaking = true;
    while (current === answer && answer.spoken < answer.phrases.length) {
        ask(answer);
        const blob = answer.mute ? null : await answer.phrases[answer.spoken].voice;
        if (current !== answer) return;
        // Остановили звук извне (проба голоса в «Персонаже») — ответ дальше не говорим.
        if (blob && !(await playToEnd(AUDIO_KEY, blob)) && current === answer) answer.mute = true;
        if (current !== answer) return;
        answer.spoken += 1;
    }
    answer.speaking = false;
    if (current === answer && answer.written) finish(answer);
}

/**
 * Всё сказано: полоса постоит и уйдёт. Микрофон открывается сам, когда шлюз сказал, что ответ спрашивает (или ответ
 * кончился «?» у шлюза постарше), после напоминания о вопросе работы и после строки работы («Делаю…», «Добавлю…»): так
 * «и ещё…» говорится без кнопки. Фразу окна, которую шлюз не принял, пульт не показывает: только подсказку.
 */
function finish(answer: Answer): void {
    current = null;
    tell();
    const done = answer.done;
    if (done?.dropped) {
        showStrip({ kind: 'note', head: tr.voice.notAnswerHead, text: tr.voice.notAnswerText }, 3000);
        return;
    }
    const said = answer.phrases.map((phrase) => phrase.text).join(' ');
    if (!said) return hideStrip();
    const why: WindowWhy | undefined = !answer.after
        ? undefined
        : done?.remind
          ? 'ready'
          : (done?.asks ?? endsWithQuestion(said))
            ? 'question'
            : done?.work
              ? 'work'
              : undefined;
    if (why && answer.after) {
        // Вопрос (или строка работы) остаётся на полосе с отсчётом, пока микрофон слушает.
        updateStrip('answer', { head: why === 'work' ? tr.voice.addMore : tr.voice.waitingAnswer, wait: ANSWER_WAIT_MS / 1000 }, KEEP_MS);
        return answer.after(why);
    }
    updateStrip('answer', { head: tr.voice.answer }, KEEP_MS);
}

/** Одна фраза своим голосом компьютера, до конца: «Готово…» работы (works.ts). false — голоса нет или оборвали. */
export async function sayAloud(text: string): Promise<boolean> {
    stopAnswer();
    try {
        const blob = await talkSpeech(text);
        return await playToEnd(AUDIO_KEY, blob);
    } catch {
        return false;
    }
}

/**
 * Фраза уходит в разговор, ответ — на полосу и, с `voice`, голосом компьютера. `after` зовётся, когда ответ договорён и
 * микрофону пора открыться самому (вопрос, напоминание, строка работы) — с причиной. Ответ назвал работу, которую фраза
 * начала или дополнила, — её «Готово» пульт скажет сам. Фраза окна (`why`) не показывается, пока шлюз её не принял.
 */
export function answerTo(turn: TalkTurn, after?: (why: WindowWhy) => void, voice = true): void {
    stopAnswer();
    const answer: Answer = { abort: new AbortController(), phrases: [], spoken: 0, written: false, mute: !voice, speaking: false, after };
    current = answer;
    tell();
    showStrip({ kind: 'answer', head: tr.voice.thinking, caps: turn.why ? undefined : turn.text });
    const fail = (message: string): void => {
        if (current !== answer) return;
        stopAnswer();
        showStrip({ kind: 'note', head: tr.voice.failedTry, caps: turn.text, text: message }, 8000);
    };
    talk({ ...turn, voice }, (event) => {
        if (current !== answer) return;
        // Начало ответа — шлюз принял фразу: теперь её можно показать.
        if (event.type === 'start') updateStrip('answer', { chip: chipOf(event.by), caps: turn.text });
        if (event.type === 'tool') updateStrip('answer', { head: tr.voice.looking(event.name) });
        if (event.type === 'phrase') {
            answer.phrases.push({ text: event.text });
            updateStrip('answer', { head: tr.voice.answering, text: answer.phrases.map((phrase) => phrase.text).join(' ') });
            ask(answer);
            void speak(answer);
        }
        if (event.type === 'error') fail(event.message);
        if (event.type === 'done') {
            if (event.work) startedHere(event.work);
            answer.done = event;
            // Вопрос работы, на который не ответили, — ещё раз, после этого ответа.
            if (event.remind) {
                answer.phrases.push({ text: event.remind });
                updateStrip('answer', { head: tr.voice.answering, text: answer.phrases.map((phrase) => phrase.text).join(' ') });
                ask(answer);
                void speak(answer);
            }
            answer.written = true;
            if (!answer.speaking) finish(answer);
        }
    }, answer.abort.signal).catch((error) => {
        if (!answer.abort.signal.aborted) fail(problemOf(error));
    });
}
