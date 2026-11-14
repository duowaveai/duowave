// Микрофон пульта (docs/plans/voice-creation.md, «Голос пульта»; решения 1–4). «Сказать» в доке открывает микрофон:
// пульт пишет фразу (16 кГц, конец — после 0,8 с тишины, не дольше 15 с) и отдаёт её «Слуху» шлюза — тому же
// распознаванию, что у колонки (`POST /api/v1/listen`): звук не уходит в Google. Слова встают на полосу после фразы
// (решение 3). Услышанное — сначала к фразам на экране (phrases.ts); не нашлось — просьба «сделай…» (та же дорога, что у
// колонки); не просьба — «Не понятно» и подсказки (решение 4). Микрофон открыт только по нажатию и ещё 6 с после
// вопроса, который пульт задал на голосовую просьбу, — сам по себе никогда. Пока открыто поле личного, он выключен.
//
// В режиме «Микрофон» (docs/plans/mic-mode.md) колонки нет, и неузнанная фраза уходит «Мозгу»: ответ — на полосу и
// голосом компьютера (speak.ts). Кончился ответ вопросом — микрофон те же 6 с слушает сам, и ответ уходит с пометкой
// «ответ». Новое «Сказать» обрывает то, что пульт говорит.

import { api, can, problemOf } from '../api';
import { tr } from '../i18n';
import { Recorder, recordingSupport } from '../recorder';
import { devMode } from '../shell/dev';
import { accusative, agentChip, askToMake } from './creation';
import { matchAction, micHeld, visiblePhrases } from './phrases';
import { ANSWER_WAIT_MS, answerTo, stopAnswer } from './speak';
import { hideStrip, requestTextToStation, showStrip, updateStrip } from './strip';
import { onComputer } from '../voice-input';

/** «Слух» шлюза ждёт 16 кГц моно. */
export const LISTEN_RATE = 16_000;
/** Конец фразы — столько тишины после речи. */
const SILENCE_END_MS = 800;
/** Фраза не длиннее. */
const MAX_MS = 15_000;
/** Нажали и молчат — столько ждём начала. */
const START_WAIT_MS = 6_000;
/** Речь — на столько дБ громче шума комнаты (его меряем первые доли секунды). */
const OVER_NOISE_DB = 12;
/** Ниже этого — тишина, каким бы тихим ни был шум. */
const FLOOR_DB = -55;

interface Session {
    recorder: Recorder | null;
    began: number;
    /** Ждём начала речи до этого времени. */
    startBy: number;
    speechAt: number | null;
    lastSpeech: number;
    noise: number[];
    ended: boolean;
    /** Пульт сам открыл микрофон (ответ на свой вопрос): без речи — закрыть молча. */
    answer: boolean;
}

let session: Session | null = null;
/** Черновики, о которых просили голосом в этом пульте: им «Готово» пульт слушает 6 с. */
const voiceDrafts = new Set<string>();
/** Где агент: облако или дома — для полосы. */
let agentCloud: boolean | null = null;
const listeners = new Set<(on: boolean) => void>();

function tell(): void {
    for (const listener of [...listeners]) listener(!!session);
}

/** Слушает ли пульт сейчас. */
export function micOn(): boolean {
    return !!session;
}

/** Слушать, открыт ли микрофон (док и полоса). */
export function onMic(listener: (on: boolean) => void): () => void {
    listeners.add(listener);
    return () => listeners.delete(listener);
}

/** Черновик сделан по голосовой просьбе в этом пульте. */
export function askedByVoice(draft: string): boolean {
    return voiceDrafts.has(draft);
}

/** Где работает агент мастерской — знает плашка; полоса пишет его. */
export function noteAgentPlace(cloud: boolean | null): void {
    agentCloud = cloud;
}

/** Микрофон выключен, пока открыто поле личного: показать это вместо слушания. */
function held(): boolean {
    if (!micHeld()) return false;
    showStrip({ kind: 'hold', head: tr.voice.micOff, text: tr.voice.micOffText }, 4000);
    return true;
}

/** «Сказать» в доке: начать слушать, а если уже слушаем — закончить фразу сейчас. */
export function toggleListening(): void {
    if (session) return void finish();
    void listen(false, START_WAIT_MS);
}

/** Пульт задал вопрос на голосовую просьбу: слушать ответ `ms` без нажатия. */
export function listenFor(ms: number): void {
    if (session) return;
    void listen(true, ms);
}

/** Закрыть микрофон без распознавания (поле ключа открылось, человек нажал «×»). */
export function cancelListening(): void {
    const current = session;
    if (!current) return;
    session = null;
    current.ended = true;
    current.recorder?.cancel();
    tell();
}

async function listen(answer: boolean, wait: number): Promise<void> {
    if (held()) return;
    // Новое «Сказать» обрывает ответ: пока пульт говорит, он себя не слушает.
    if (!answer) stopAnswer();
    const support = recordingSupport();
    if (!support.ok) {
        showStrip({ kind: 'note', head: tr.voice.noMic, text: support.reason }, 8000);
        return;
    }
    const now = Date.now();
    const current: Session = { recorder: null, began: now, startBy: now + wait, speechAt: null, lastSpeech: now, noise: [], ended: false, answer };
    session = current;
    tell();
    if (!answer) showStrip({ kind: 'listen', head: tr.voice.listening, chip: { text: tr.voice.heardHere, tone: 'own' }, level: 0 });
    try {
        current.recorder = await Recorder.start(undefined, (db) => level(current, db), { rate: LISTEN_RATE, processed: true });
        if (current.ended) current.recorder.cancel();
    } catch (error) {
        if (session === current) session = null;
        tell();
        showStrip({ kind: 'note', head: tr.voice.micFailed, text: error instanceof Error ? error.message : String(error) }, 8000);
    }
}

/** Уровень звука: речь началась, речь кончилась (0,8 с тишины), фраза слишком длинная, никто не заговорил. */
function level(current: Session, db: number): void {
    if (current.ended || session !== current) return;
    if (micHeld()) {
        cancelListening();
        held();
        return;
    }
    const now = Date.now();
    if (current.noise.length < 8 && current.speechAt === null) current.noise.push(db);
    const noise = current.noise.length ? current.noise.reduce((a, b) => a + b, 0) / current.noise.length : FLOOR_DB;
    const speaking = db > Math.max(FLOOR_DB, noise + OVER_NOISE_DB);
    if (speaking) {
        if (current.speechAt === null) current.speechAt = now;
        current.lastSpeech = now;
        if (current.answer) showStrip({ kind: 'listen', head: tr.voice.listeningAnswer, chip: { text: tr.voice.heardHere, tone: 'own' } });
    }
    updateStrip('listen', { level: Math.max(0, Math.min(1, (db - FLOOR_DB) / 45)) });
    const long = now - current.began > MAX_MS;
    const done = current.speechAt !== null && now - current.lastSpeech > SILENCE_END_MS;
    const nobody = current.speechAt === null && now > current.startBy;
    if (nobody) {
        cancelListening();
        if (current.answer) return;
        showStrip({ kind: 'note', head: tr.voice.notHeard, text: tr.voice.notHeardText }, 4000);
        return;
    }
    if (done || long) void finish();
}

/** Фраза кончилась: распознать и сделать, что сказано. */
async function finish(): Promise<void> {
    const current = session;
    if (!current || current.ended) return;
    current.ended = true;
    session = null;
    tell();
    const recorder = current.recorder;
    if (!recorder) return;
    if (current.speechAt === null) {
        recorder.cancel();
        if (!current.answer) hideStrip();
        return;
    }
    showStrip({ kind: 'listen', head: tr.voice.recognizing, chip: { text: tr.voice.heardHere, tone: 'own' } });
    try {
        const { wav } = await recorder.stop();
        const asked = Date.now();
        const heard = await api.listen(wav);
        const ms = Date.now() - asked;
        const chip = heard.heard === 'cloud' ? { text: tr.voice.heardCloud, tone: 'out' as const } : { text: tr.voice.heardHere, tone: 'own' as const };
        const text = heard.text.trim();
        if (!text) {
            showStrip({ kind: 'note', head: tr.voice.notCaught, chip, text: tr.voice.notCaughtText }, 5000);
            return;
        }
        showStrip({ kind: 'listen', head: tr.voice.heard, chip, caps: text }, 4000);
        act(text, chip, { by: heard.heard, ms }, current.answer);
    } catch (error) {
        showStrip({ kind: 'note', head: tr.voice.notRecognized, text: problemOf(error) }, 6000);
    }
}

/**
 * Что сказано: фраза кнопки на экране, просьба «сделай…», а остальное — «Мозгу» в режиме «Микрофон» и «Не понятно» в
 * режиме «Колонка» (там отвечает колонка). `answered` — фраза сказана в ответ на вопрос, без нажатия.
 */
function act(text: string, chip: { text: string; tone: 'own' | 'out' }, heard?: { by: 'home' | 'cloud'; ms?: number }, answered = false): void {
    const action = matchAction(text);
    if (action) return action.run();
    if (/^((алиса|alice)[\s,]+)?(сделай|создай|нарисуй|напиши|давай сделаем|давай создадим|добавь|make|create|draw|write|add|build)\s/i.test(text.trim()) && can('plugins.manage')) {
        void askToMake(text, agentCloud)
            .then((made) => {
                voiceDrafts.add(made.draft);
                showStrip(
                    {
                        kind: 'agent',
                        head: tr.voice.agent,
                        chip: agentChip(agentCloud),
                        caps: text,
                        text: tr.voice.makingNew(accusative(made.kind)),
                    },
                    6000,
                );
            })
            .catch((error) => showStrip({ kind: 'note', head: tr.voice.failedTry, caps: text, text: problemOf(error) }, 8000));
        return;
    }
    if (onComputer()) {
        // Может ли этот вход говорить (`use:chat`), знает шлюз: откажет — его слова встанут на полосу.
        answerTo({ text, via: answered ? 'answer' : 'button', heard }, () => listenFor(ANSWER_WAIT_MS));
        return;
    }
    const hints = visiblePhrases().slice(0, 6);
    showStrip(
        {
            kind: 'note',
            head: tr.voice.notUnderstood,
            chip,
            caps: text,
            text: tr.voice.notUnderstoodText,
            chips: [...hints.map((label) => ({ label })), { label: tr.voice.textToSpeaker, act: requestTextToStation }],
        },
        8000,
    );
}

/** Услышанные слова, как их сделает пульт (без микрофона): проверить путь голосом, не говоря вслух. */
export function hearWords(text: string): void {
    showStrip({ kind: 'listen', head: tr.voice.heard, chip: { text: tr.voice.heardHere, tone: 'own' }, caps: text }, 4000);
    act(text, { text: tr.voice.heardHere, tone: 'own' }, { by: 'home' });
}

// Слой разработчика («Настройки» → «Для разработчиков»): `__hear('примерить')` в консоли — как сказать это пульту.
if (devMode()) (window as unknown as { __hear?: (text: string) => void }).__hear = hearWords;
