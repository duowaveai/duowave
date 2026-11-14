import { Place, RequestPhase, RequestView } from '../api';
import { joinRu, LIT, NodeKey, NodeView, Tone, TripStage } from './dash-model';
import { device } from '../words';
import { formatNumber, tr } from '../i18n';

// Живой путь вопроса на дашборде: настоящий запрос колонки (живой канал шлюза) как шаги орбиты — кто сейчас
// работает (цвет кометы и кольца), какие узлы горят, что сказать под колонкой. Без отрисовки.

/** Сколько законченный запрос виден на орбите: его итог, потом снова покой. */
export const DONE_MS = 6000;

/** Шаг орбиты для шага запроса: слух слушает и распознаёт, мозг думает и наводит справки, голос отвечает. */
export function stageOf(phase: RequestPhase | undefined): TripStage {
    switch (phase) {
        case 'listen':
        case 'recognize':
            return 'listen';
        case 'speak':
            return 'speak';
        default:
            // Услышано и ждёт исполнителя, думает, смотрит: всё это — мозг.
            return 'think';
    }
}

export function toneOf(by: Place | undefined): Tone | undefined {
    return by;
}

/** Где был шаг, по-человечески: для «Наружу: мозг и голос». */
const nodeOf = (): Record<RequestPhase, string> => ({
    listen: tr.trip.nodeEar,
    recognize: tr.trip.nodeEar,
    think: tr.trip.nodeBrain,
    tool: tr.trip.nodeTool,
    speak: tr.trip.nodeVoice,
    play: tr.trip.nodePlayer,
});

/** Что из запроса уходило из дома: шаги облака и Яндекса. */
export function outsideOfTrip(request: RequestView): string[] {
    const outside = request.steps.filter((step) => step.by === 'cloud' || step.by === 'yandex').map((step) => nodeOf()[step.phase]);
    return [...new Set(outside)];
}

/** «1,8 с», «0,4 с», «12 с». */
export function seconds(ms: number): string {
    const value = ms / 1000;
    return tr.trip.secondsShort(value < 10 ? formatNumber(value, { minimumFractionDigits: 1, maximumFractionDigits: 1 }) : formatNumber(Math.round(value)));
}

const quote = (text: string | undefined): string | undefined => (text ? tr.trip.quoted(text.replace(/[.!?…]+$/u, '')) : undefined);

/** Почему прервано: то, что говорит журнал, словами пульта. */
function cancelReason(error: string | undefined): string {
    if (!error) return '';
    if (/superseded/.test(error)) return tr.trip.cancelSuperseded;
    if (/disconnected/.test(error)) return tr.trip.cancelDisconnected;
    if (/gateway stopped/.test(error)) return tr.trip.cancelGatewayStopped;
    if (/cancel/.test(error)) return tr.trip.cancelByStation;
    return '';
}

export interface LiveTrip {
    stage: TripStage;
    /** Кто работает на этом шаге: цвет кометы и кольца; нет — цвет узла. */
    tone?: Tone;
    lit: NodeKey[];
    caption: [string, string];
    /** Для чтения с экрана: шаг целиком. */
    label: string;
}

/**
 * Запрос на орбите: идущий — его шаг; законченный — итог DONE_MS после конца (по часам шлюза: `now` уже со
 * сдвигом); старше — ничего, орбита в покое. Без права на журнал слов нет — подпись говорит, кто работает.
 */
export function liveTripOf(request: RequestView | undefined, now: number, nodes: Record<NodeKey, NodeView>): LiveTrip | undefined {
    if (!request) return undefined;
    if (request.status === 'active') {
        const step = request.steps.at(-1);
        const open = step && step.ms === undefined ? step : undefined;
        const stage = stageOf(request.phase);
        const tone = toneOf(open?.by);
        const lit = LIT[stage];
        switch (request.phase) {
            case 'listen':
                return {
                    stage,
                    tone,
                    lit,
                    caption: [tr.trip.listening, quote(request.partial) ?? tr.trip.earLine(open?.what ?? nodes.ear.value)],
                    label: tr.trip.labelListening,
                };
            case 'recognize':
                return {
                    stage,
                    tone,
                    lit,
                    caption: [tr.trip.recognizing, quote(request.partial) ?? tr.trip.earLine(open?.what ?? nodes.ear.value)],
                    label: tr.trip.labelRecognizing,
                };
            case 'tool':
                return {
                    stage,
                    tone,
                    lit: ['brain'],
                    caption: [tr.trip.looking, `${open?.what ?? tr.trip.toolDefault}${open?.by === 'yandex' ? tr.trip.yandexSuffix : ''}`],
                    label: open?.what ? tr.trip.labelToolWhat(open.what) : tr.trip.labelTool,
                };
            case 'speak':
                return {
                    stage,
                    tone,
                    lit,
                    caption: [tr.trip.answering, quote(request.answer) ?? tr.trip.speaksLine(open?.what ?? nodes.voice.value)],
                    label: tr.trip.labelAnswering(device('nom', true)),
                };
            default: {
                const who = open?.by === 'yandex' ? tr.trip.yandexThinks : (open?.what ?? nodes.brain.value);
                return {
                    stage,
                    tone,
                    lit,
                    caption: [tr.trip.thinking, quote(request.text) ?? who],
                    label: tr.trip.labelThinking(who),
                };
            }
        }
    }
    const endedAt = Date.parse(request.startedAt) + (request.ms ?? 0);
    if (now - endedAt > DONE_MS) return undefined;
    switch (request.status) {
        case 'answered': {
            const out = outsideOfTrip(request);
            const sub = request.mode === 'relay' ? tr.trip.allAnsweredByYandex : out.length ? tr.trip.outsideTo(joinRu(out)) : tr.trip.allStayedHome;
            // Что чувствует человек — через сколько колонка заговорила после того, как он договорил (или написал);
            // без речи — сколько шёл весь запрос.
            const spoke = request.steps.find((step) => step.phase === 'speak');
            const listen = request.steps.find((step) => step.phase === 'listen');
            const said = listen?.ms !== undefined ? listen.at + listen.ms : 0;
            const title = spoke
                ? tr.trip.answerIn(seconds(Math.max(0, spoke.at - said)))
                : request.ms !== undefined
                  ? tr.trip.doneIn(seconds(request.ms))
                  : tr.trip.done;
            return { stage: 'done', lit: [], caption: [title, sub], label: `${title}. ${sub}` };
        }
        case 'silent':
            return {
                stage: 'done',
                lit: [],
                // У колонки тишина после её слова — не расслышала; у разговора без колонки фраза уже распознана.
                caption: request.kind === 'voice' && request.device !== 'computer' ? [tr.trip.didntCatch, tr.trip.silenceAfterWake] : [tr.trip.noAnswer, tr.trip.nothingToSay],
                label: tr.trip.labelNoAnswer,
            };
        case 'cancelled': {
            const why = cancelReason(request.error);
            return { stage: 'done', lit: [], caption: [tr.trip.cancelled, why], label: why ? tr.trip.labelCancelled(why) : tr.trip.cancelled };
        }
        default:
            return { stage: 'done', lit: [], caption: [tr.trip.failed, tr.trip.detailsInJournal], label: tr.trip.labelFailed };
    }
}

// Для журнала: слова для шагов, мест, итогов.

/**
 * Что чувствует человек: через сколько колонка заговорила после того, как он договорил (или написал); без речи —
 * сколько шёл весь запрос. Нет ни того, ни другого — undefined (запрос ещё идёт).
 */
export function feltMs(request: RequestView): number | undefined {
    const spoke = request.steps.find((step) => step.phase === 'speak');
    const listen = request.steps.find((step) => step.phase === 'listen');
    const said = listen?.ms !== undefined ? listen.at + listen.ms : 0;
    if (spoke) return Math.max(0, spoke.at - said);
    return request.ms;
}

export const PHASE_WORDS: Record<RequestPhase, string> = {
    get listen() {
        return tr.trip.phaseListen;
    },
    get recognize() {
        return tr.trip.phaseRecognize;
    },
    get think() {
        return tr.trip.phaseThink;
    },
    get tool() {
        return tr.trip.phaseTool;
    },
    get speak() {
        return tr.trip.phaseSpeak;
    },
    get play() {
        return tr.trip.phasePlay;
    },
};

export const STEP_TITLES: Record<RequestPhase, string> = {
    get listen() {
        return tr.trip.stepListen;
    },
    get recognize() {
        return tr.trip.stepRecognize;
    },
    get think() {
        return tr.trip.stepThink;
    },
    get tool() {
        return tr.trip.stepTool;
    },
    get speak() {
        return tr.trip.stepSpeak;
    },
    // Что колонка сказала о включённом: начала играть, молчит, остановила (шлюз, этап 5а).
    get play() {
        return tr.trip.stepPlay;
    },
};

export const PLACE_WORDS: Record<Place, string> = {
    get own() {
        return tr.trip.placeOwn;
    },
    get cloud() {
        return tr.trip.placeCloud;
    },
    get yandex() {
        return tr.trip.placeYandex;
    },
    get agent() {
        return tr.trip.placeAgent;
    },
};

export const KIND_WORDS: Record<RequestView['kind'], string> = {
    get voice() {
        return tr.trip.kindVoice;
    },
    get text() {
        return tr.trip.kindText;
    },
    get say() {
        return tr.trip.kindSay;
    },
};

/** Кто ответил — пилюлей: цвет и слово. */
export function answeredBy(request: RequestView): { tone: Tone; label: string } | undefined {
    switch (request.executor) {
        case 'yandex':
            return { tone: 'yandex', label: tr.trip.yandex };
        case 'model': {
            const think = request.steps.filter((step) => step.phase === 'think').at(-1);
            return think?.by === 'cloud' ? { tone: 'cloud', label: tr.trip.modelCloud } : { tone: 'own', label: tr.trip.modelHome };
        }
        case 'own':
            return { tone: 'own', label: tr.trip.gatewaySkill };
        case 'unsupported':
            return { tone: 'off', label: tr.trip.notYet };
        default:
            return request.kind === 'say' ? { tone: 'own', label: tr.trip.gateway } : undefined;
    }
}

/** Итог запроса пилюлей: класс и слово; идущий — его шаг. */
export function statusOf(request: RequestView): { cls: string; label: string } {
    switch (request.status) {
        case 'active':
            return { cls: 'pill-own req-going', label: request.phase ? PHASE_WORDS[request.phase] : tr.trip.going };
        case 'answered':
            return { cls: 'pill-off', label: request.ms !== undefined ? seconds(request.ms) : tr.trip.answer };
        case 'silent':
            return { cls: 'pill-off', label: tr.trip.silence };
        case 'cancelled':
            return { cls: 'pill-off', label: tr.trip.cancelledShort };
        default:
            return { cls: 'pill-error', label: tr.trip.error };
    }
}
