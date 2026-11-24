// Микрофон пульта (docs/plans/voice-creation.md, «Голос пульта»; решения 1–3). «Сказать» в доке открывает микрофон:
// пульт пишет мысль (16 кГц) кусками, отрезанными на паузах, и отдаёт каждый «Слуху» шлюза — тому же распознаванию,
// что у колонки (`POST /api/v1/listen`): звук не уходит в Google. Шлюз судит кусок словами языка дома
// (docs/plans/audio-interface-2.md, «К1. Мысль»): обычная фраза кончается после 0,8 с тишины, оборванная на «и…»,
// «чтобы…» — после 3,5 с, «подожди» — после 20 с, «да», «стоп» — сразу; потолок мысли — минута. Недосказанное после
// тишины не уходит обрывком: ждёт две минуты, следующая «Сказать» склеивается с ним. Слова встают на полосу после фразы
// (решение 3). Услышанное — сначала к фразам на экране (phrases.ts); остальное уходит в разговор
// (docs/plans/audio-interface.md, решение 6): правила слоя — «сделай виджет…», «как там виджет», дверь мастерской —
// без модели, прочее — «Мозгу»; своего шаблона «сделай…» у пульта больше нет. Ответ — на полосу; в режиме «Микрофон»
// (docs/plans/mic-mode.md) ещё и голосом компьютера (speak.ts), с колонкой отвечает колонка. Микрофон открыт только по
// нажатию и ещё 6 с после вопроса — ответа «Мозга» или «Готово…» работы (works.ts), — сам по себе никогда; ответ уходит
// с пометкой «ответ». Пока открыто поле личного, он выключен. Новое «Сказать» обрывает то, что пульт говорит.

import { Thought, WindowWhy, api, problemOf } from '../api';
import { tr } from '../i18n';
import { Recorder, recordingSupport } from '../recorder';
import { devMode } from '../shell/dev';
import { matchAction, micHeld } from './phrases';
import { ANSWER_WAIT_MS, answerTo, sayAloud, stopAnswer } from './speak';
import { StripMessage, hideStrip, onStripClosed, showStrip, stripNow, updateStrip } from './strip';
import { computerVoice, onComputer } from '../voice-input';
import { chosenInput } from './devices';
import { LISTEN_TONE_MS, playTone } from './tones';

/** «Слух» шлюза ждёт 16 кГц моно. */
export const LISTEN_RATE = 16_000;
/** Тишина, на которой пульт отрезает кусок мысли и отдаёт его «Слуху», а сам слушает дальше. */
const PAUSE_MS = 400;
/** Кусок без паузы режется силой: «Слух» берёт фразу не длиннее 15 с. */
const PIECE_MAX_MS = 14_000;
/** Сколько ждать тишины и потолок мысли, пока шлюз не сказал своё (шлюз постарше — как раньше). */
const PLAIN_MS = 800;
const MAX_MS = 15_000;
/** Нажали и молчат — столько ждём начала. */
const START_WAIT_MS = 6_000;
/** Речь — на столько дБ громче шума комнаты (его меряем первые доли секунды). */
const OVER_NOISE_DB = 12;
/** Ниже этого — тишина, каким бы тихим ни был шум. */
const FLOOR_DB = -55;

/** Чем кончилась мысль: кнопкой, тишиной, потолком или коротким словом. */
type Ending = 'button' | 'silence' | 'cap' | 'short';

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
    /** Слушаем ответ на вопрос работы («На стол или примерить?»): он идёт слою, мимо кнопок на экране. */
    work: boolean;
    /** Почему микрофон открылся сам: фраза — закрытого окна (docs/plans/audio-interface-2.md, «К2»). */
    why?: WindowWhy;
    /** До этого времени звучит «слушаю»: звук не считается ни шумом, ни речью. */
    quietUntil: number;
    /** Когда началась речь куска, ещё не отданного «Слуху»; null — всё сказанное отдано. */
    pieceAt: number | null;
    /** Сколько кусков отдано. */
    pieces: number;
    /** Куски распознаются по одному, по порядку. */
    queue: Promise<void>;
    /** Сколько кусков ещё в распознавании. */
    pending: number;
    texts: string[];
    /** Сколько ждать тишины после речи — по последнему куску (шлюз судит его словами языка дома). */
    wait: number;
    end: Thought['end'];
    max: number;
    park: number;
    by: 'home' | 'cloud';
    ms: number;
    failure: string | null;
}

let session: Session | null = null;
const listeners = new Set<(on: boolean) => void>();

/** Недосказанная мысль ждёт продолжения: следующая «Сказать» склеится с ней. */
let parked: { text: string; until: number; message: StripMessage } | null = null;

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

/** Микрофон выключен, пока открыто поле личного: показать это вместо слушания. */
function held(): boolean {
    if (!micHeld()) return false;
    showStrip({ kind: 'hold', head: tr.voice.micOff, text: tr.voice.micOffText }, 4000);
    return true;
}

/** «Сказать» в доке: начать слушать, а если уже слушаем — закончить мысль сейчас. */
export function toggleListening(): void {
    if (session) return void finish(session, 'button');
    void listen(false, START_WAIT_MS);
}

/**
 * Микрофон открывается сам на `ms`: пульт задал вопрос («Мозг» спросил, «Готово…» работы) или сказал строку работы
 * («Делаю…»). `why` — почему: фраза уходит в закрытое окно шлюза (решения 4 и 5 плана второго этапа); ответ работе идёт
 * слою мимо кнопок на экране.
 */
export function listenFor(ms: number, why: WindowWhy = 'question'): void {
    if (session) return;
    void listen(true, ms, why);
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

// «×» на полосе с недосказанным — забыть его.
onStripClosed(() => {
    if (parked && stripNow() === parked.message) parked = null;
});

async function listen(answer: boolean, wait: number, why?: WindowWhy): Promise<void> {
    if (held()) return;
    // Новое «Сказать» обрывает ответ: пока пульт говорит, он себя не слушает.
    if (!answer) stopAnswer();
    const support = recordingSupport();
    if (!support.ok) {
        // Пульт открыт по сети (телефон, http://колонка): микрофона браузер не даст — сказать, где говорить, и подольше
        // (было «не работает» — тест 21.11).
        const text = window.isSecureContext ? support.reason : tr.voice.micOnlyOnComputer;
        showStrip({ kind: 'note', head: tr.voice.noMic, text }, window.isSecureContext ? 8000 : 15000);
        return;
    }
    const now = Date.now();
    const current: Session = {
        recorder: null,
        began: now,
        startBy: now + wait,
        speechAt: null,
        lastSpeech: now,
        noise: [],
        ended: false,
        answer,
        work: why === 'ready' || why === 'work',
        why,
        quietUntil: now + LISTEN_TONE_MS + 30,
        pieceAt: null,
        pieces: 0,
        queue: Promise.resolve(),
        pending: 0,
        texts: [],
        wait: PLAIN_MS,
        end: 'plain',
        max: MAX_MS,
        park: 0,
        by: 'home',
        ms: 0,
        failure: null,
    };
    session = current;
    tell();
    playTone('listen');
    if (!answer) showStrip({ kind: 'listen', head: tr.voice.listening, chip: { text: tr.voice.heardHere, tone: 'own' }, level: 0, caps: parkedCaps() });
    try {
        current.recorder = await startRecorder((db) => level(current, db));
        if (current.ended) current.recorder.cancel();
    } catch (error) {
        if (session === current) session = null;
        tell();
        showStrip({ kind: 'note', head: tr.voice.micFailed, text: error instanceof Error ? error.message : String(error) }, 8000);
    }
}

/** Запись с выбранного микрофона (voice/devices.ts); его нет — с системного. */
export async function startRecorder(onLevel: (db: number) => void): Promise<Recorder> {
    const chosen = chosenInput();
    if (chosen) {
        try {
            return await Recorder.start(chosen, onLevel, { rate: LISTEN_RATE, processed: true });
        } catch {
            // Выбранного нет (отключили) — системный.
        }
    }
    return Recorder.start(undefined, onLevel, { rate: LISTEN_RATE, processed: true });
}

/** Недосказанное, которое ждёт, — крупно на полосе, пока слушаем продолжение. */
function parkedCaps(): string | undefined {
    if (parked && parked.until < Date.now()) parked = null;
    return parked ? `${parked.text}…` : undefined;
}

/**
 * Уровень звука. Речь началась — кусок начат; тишина `PAUSE_MS` — кусок отрезан и ушёл «Слуху», а пульт слушает
 * дальше; мысль кончилась, когда после речи прошло столько тишины, сколько велел последний кусок, и все куски
 * распознаны. Потолок, никто не заговорил — как раньше.
 */
function level(current: Session, db: number): void {
    if (current.ended || session !== current) return;
    if (micHeld()) {
        cancelListening();
        held();
        return;
    }
    const now = Date.now();
    // Пока звучит «слушаю», микрофон слышит его: не шум и не речь.
    if (now < current.quietUntil) return;
    if (current.noise.length < 8 && current.speechAt === null) current.noise.push(db);
    const noise = current.noise.length ? current.noise.reduce((a, b) => a + b, 0) / current.noise.length : FLOOR_DB;
    const speaking = db > Math.max(FLOOR_DB, noise + OVER_NOISE_DB);
    const head = current.answer ? tr.voice.listeningAnswer : tr.voice.listening;
    if (speaking) {
        if (current.speechAt === null) current.speechAt = now;
        if (current.pieceAt === null) {
            current.pieceAt = now;
            if (current.answer || current.pieces) showStrip({ kind: 'listen', head, chip: { text: tr.voice.heardHere, tone: 'own' }, caps: parkedCaps() });
        }
        current.lastSpeech = now;
    }
    updateStrip('listen', { level: Math.max(0, Math.min(1, (db - FLOOR_DB) / 45)) });
    const nobody = current.speechAt === null && now > current.startBy;
    if (nobody) {
        cancelListening();
        if (current.answer) return;
        showStrip({ kind: 'note', head: tr.voice.notHeard, text: tr.voice.notHeardText }, 4000);
        return;
    }
    const quiet = now - current.lastSpeech;
    if (current.pieceAt !== null && (quiet >= PAUSE_MS || now - current.pieceAt > PIECE_MAX_MS)) cut(current);
    if (now - current.began > current.max) return void finish(current, 'cap');
    if (current.speechAt === null || current.pieceAt !== null) return;
    // Тишина после «и…», «чтобы…» или «подожди»: ждём дольше и говорим об этом.
    if (current.end === 'hanging' || current.end === 'wait') {
        const left = Math.max(0, Math.ceil((current.wait - quiet) / 1000));
        updateStrip('listen', { head: current.end === 'wait' ? tr.voice.waitMore : tr.voice.goOn, wait: left });
    }
    if (current.pending === 0 && quiet >= current.wait) void finish(current, 'silence');
}

/** Отрезать сказанное с прошлой паузы и отдать «Слуху» в очередь; запись идёт дальше. */
function cut(current: Session): void {
    const recorder = current.recorder;
    if (!recorder || current.pieceAt === null) return;
    current.pieceAt = null;
    const piece = ++current.pieces;
    const taken = recorder.take();
    current.pending += 1;
    current.queue = current.queue.then(async () => {
        try {
            const { wav } = await taken;
            const asked = Date.now();
            const heard = await api.listen(wav, piece);
            current.ms += Date.now() - asked;
            if (heard.heard === 'cloud') current.by = 'cloud';
            const text = heard.text.trim();
            if (!text) return;
            const thought = heard.thought;
            if (!thought) {
                current.texts.push(text);
                return;
            }
            if (thought.text.trim()) current.texts.push(thought.text.trim());
            current.wait = thought.waitMs;
            current.end = thought.end;
            current.max = thought.maxMs;
            current.park = thought.parkMs;
        } catch (error) {
            current.failure ??= problemOf(error);
        } finally {
            current.pending -= 1;
        }
        // «Да», «стоп» первой фразой — мысль кончилась, как только распознана.
        if (current.end === 'short' && current.pieceAt === null && session === current) void finish(current, 'short');
    });
}

/** Мысль кончилась: дораспознать последний кусок, склеить и сделать, что сказано — или отложить недосказанное. */
async function finish(current: Session, ending: Ending): Promise<void> {
    if (current.ended) return;
    current.ended = true;
    if (session === current) session = null;
    tell();
    const recorder = current.recorder;
    if (!recorder) return;
    if (current.speechAt === null) {
        recorder.cancel();
        if (!current.answer) hideStrip();
        return;
    }
    if (current.pieceAt !== null) cut(current);
    if (current.pending) showStrip({ kind: 'listen', head: tr.voice.recognizing, chip: { text: tr.voice.heardHere, tone: 'own' }, caps: parkedCaps() });
    await current.queue;
    recorder.cancel();
    if (current.failure && !current.texts.length) {
        showStrip({ kind: 'note', head: tr.voice.notRecognized, text: current.failure }, 6000);
        return;
    }
    const chip = current.by === 'cloud' ? { text: tr.voice.heardCloud, tone: 'out' as const } : { text: tr.voice.heardHere, tone: 'own' as const };
    const said = current.texts.join(' ').trim();
    const before = parkedCaps() !== undefined ? parked : null;
    if (!said) {
        if (before) return void showStrip(before.message, Math.max(1, before.until - Date.now()));
        showStrip({ kind: 'note', head: tr.voice.notCaught, chip, text: tr.voice.notCaughtText }, 5000);
        return;
    }
    // Короткое слово одно («стоп», «да») недосказанное не продолжает — и снимает его.
    const alone = current.end === 'short' && current.pieces === 1;
    parked = null;
    const text = before && !alone ? `${before.text} ${said}` : said;
    // Тишина после «чтобы…» или «подожди»: мысль не кончена — ждёт продолжения, а не уходит обрывком.
    if (ending === 'silence' && !current.answer && (current.end === 'hanging' || current.end === 'wait') && current.park > 0) {
        return park(text, current.park);
    }
    playTone('heard');
    // Фраза окна (без нажатия) не показывается, пока шлюз её не принял.
    showStrip({ kind: 'listen', head: tr.voice.heard, chip, caps: current.answer ? undefined : text }, 4000);
    act(text, { by: current.by, ms: current.ms }, current.answer ? (current.why ?? 'question') : undefined);
}

/** Недосказанное ждёт `ms`: на полосе с «×», в режиме «Микрофон» — «Жду.» голосом. */
function park(text: string, ms: number): void {
    const message: StripMessage = { kind: 'note', head: tr.voice.parkedHead, caps: `${text}…`, text: tr.voice.parkedText };
    parked = { text, until: Date.now() + ms, message };
    showStrip(message, ms);
    if (onComputer()) void sayAloud(tr.voice.waitWord);
}

/**
 * Что сказано: фраза кнопки на экране, а остальное — в разговор (решение 6 аудиоинтерфейса): правила слоя и «Мозг»
 * отвечают на полосе и голосом компьютера — в режиме «Микрофон» всегда, в режиме «Колонка» — когда голос дома свой
 * (решение владельца к находке 9 проверки шага 11: где спросили, там и ответ; голос Алисы здесь не звучит — текстом). `why` — фраза сказана без нажатия, в окне после нашей
 * реплики: она идёт шлюзу как есть (его закрытое окно решает, принять ли), кнопки на экране её не перехватывают —
 * разве что после вопроса «Мозга», как и раньше. После ответа микрофон снова открывается сам, если шлюз так сказал.
 */
function act(text: string, heard?: { by: 'home' | 'cloud'; ms?: number }, why?: WindowWhy): void {
    if (!why || why === 'question') {
        const action = matchAction(text);
        if (action) return action.run();
    }
    // Может ли этот вход говорить (`use:chat`), знает шлюз: откажет — его слова встанут на полосу.
    const voice = onComputer() || computerVoice();
    answerTo({ text, via: why ? 'answer' : 'button', why, heard }, voice ? (reason) => listenFor(ANSWER_WAIT_MS, reason) : undefined, voice);
}

/** Услышанные слова, как их сделает пульт (без микрофона): проверить путь голосом, не говоря вслух. */
export function hearWords(text: string): void {
    showStrip({ kind: 'listen', head: tr.voice.heard, chip: { text: tr.voice.heardHere, tone: 'own' }, caps: text }, 4000);
    act(text, { by: 'home' });
}

// Слой разработчика («Настройки» → «Для разработчиков»): `__hear('примерить')` в консоли — как сказать это пульту.
if (devMode()) (window as unknown as { __hear?: (text: string) => void }).__hear = hearWords;
