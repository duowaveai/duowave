import { Place, RequestPhase, RequestView } from '../api';
import { joinRu, LIT, NodeKey, NodeView, Tone, TripStage } from './dash-model';

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
const NODE_OF: Record<RequestPhase, string> = { listen: 'слух', recognize: 'слух', think: 'мозг', tool: 'справка', speak: 'голос', play: 'плеер' };

/** Что из запроса уходило из дома: шаги облака и Яндекса. */
export function outsideOfTrip(request: RequestView): string[] {
    const outside = request.steps.filter((step) => step.by === 'cloud' || step.by === 'yandex').map((step) => NODE_OF[step.phase]);
    return [...new Set(outside)];
}

/** «1,8 с», «0,4 с», «12 с». */
export function seconds(ms: number): string {
    const value = ms / 1000;
    return `${value < 10 ? value.toFixed(1).replace('.', ',') : Math.round(value)} с`;
}

const quote = (text: string | undefined): string | undefined => (text ? `«${text.replace(/[.!?…]+$/u, '')}»` : undefined);

/** Почему прервано: то, что говорит журнал, словами пульта. */
function cancelReason(error: string | undefined): string {
    if (!error) return '';
    if (/superseded/.test(error)) return 'новым вопросом';
    if (/disconnected/.test(error)) return 'колонка отключилась';
    if (/gateway stopped/.test(error)) return 'шлюз остановился';
    if (/cancel/.test(error)) return 'колонка отменила';
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
                    caption: ['Слушает…', quote(request.partial) ?? `слух: ${open?.what ?? nodes.ear.value}`],
                    label: 'Колонка слушает',
                };
            case 'recognize':
                return {
                    stage,
                    tone,
                    lit,
                    caption: ['Распознаёт…', quote(request.partial) ?? `слух: ${open?.what ?? nodes.ear.value}`],
                    label: 'Колонка распознаёт сказанное',
                };
            case 'tool':
                return {
                    stage,
                    tone,
                    lit: ['brain'],
                    caption: ['Смотрит…', `${open?.what ?? 'справка'}${open?.by === 'yandex' ? ' · Яндекс' : ''}`],
                    label: `Мозг наводит справку${open?.what ? `: ${open.what}` : ''}`,
                };
            case 'speak':
                return {
                    stage,
                    tone,
                    lit,
                    caption: ['Отвечает…', quote(request.answer) ?? `говорит ${open?.what ?? nodes.voice.value}`],
                    label: 'Колонка отвечает',
                };
            default: {
                const who = open?.by === 'yandex' ? 'думает Алиса' : (open?.what ?? nodes.brain.value);
                return {
                    stage,
                    tone,
                    lit,
                    caption: ['Думает…', quote(request.text) ?? who],
                    label: `Думает: ${who}`,
                };
            }
        }
    }
    const endedAt = Date.parse(request.startedAt) + (request.ms ?? 0);
    if (now - endedAt > DONE_MS) return undefined;
    switch (request.status) {
        case 'answered': {
            const out = outsideOfTrip(request);
            const sub = request.mode === 'relay' ? 'всё ответил Яндекс' : out.length ? `наружу: ${joinRu(out)}` : 'всё осталось дома';
            // Что чувствует человек — через сколько колонка заговорила после того, как он договорил (или написал);
            // без речи — сколько шёл весь запрос.
            const spoke = request.steps.find((step) => step.phase === 'speak');
            const listen = request.steps.find((step) => step.phase === 'listen');
            const said = listen?.ms !== undefined ? listen.at + listen.ms : 0;
            const title = spoke
                ? `Ответ через ${seconds(Math.max(0, spoke.at - said))}`
                : request.ms !== undefined
                  ? `Готово за ${seconds(request.ms)}`
                  : 'Готово';
            return { stage: 'done', lit: [], caption: [title, sub], label: `${title}. ${sub}` };
        }
        case 'silent':
            return {
                stage: 'done',
                lit: [],
                caption: request.kind === 'voice' ? ['Не расслышала', 'после «Алиса» — тишина'] : ['Без ответа', 'сказать было нечего'],
                label: 'Ответа не было',
            };
        case 'cancelled': {
            const why = cancelReason(request.error);
            return { stage: 'done', lit: [], caption: ['Прервано', why], label: `Прервано${why ? `: ${why}` : ''}` };
        }
        default:
            return { stage: 'done', lit: [], caption: ['Не получилось', 'подробности — в журнале'], label: 'Запрос не удался' };
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
    listen: 'слушает',
    recognize: 'распознаёт',
    think: 'думает',
    tool: 'смотрит',
    speak: 'отвечает',
    play: 'играет',
};

export const STEP_TITLES: Record<RequestPhase, string> = {
    listen: 'Слушала',
    recognize: 'Распознала',
    think: 'Думала',
    tool: 'Справка',
    speak: 'Говорила',
    // Что колонка сказала о включённом: начала играть, молчит, остановила (шлюз, этап 5а).
    play: 'Плеер',
};

export const PLACE_WORDS: Record<Place, string> = { own: 'дома', cloud: 'облако', yandex: 'Яндекс', agent: 'помощник' };

export const KIND_WORDS: Record<RequestView['kind'], string> = { voice: 'голосом', text: 'текстом', say: 'сказала сама' };

/** Кто ответил — пилюлей: цвет и слово. */
export function answeredBy(request: RequestView): { tone: Tone; label: string } | undefined {
    switch (request.executor) {
        case 'yandex':
            return { tone: 'yandex', label: 'Яндекс' };
        case 'model': {
            const think = request.steps.filter((step) => step.phase === 'think').at(-1);
            return think?.by === 'cloud' ? { tone: 'cloud', label: 'модель · облако' } : { tone: 'own', label: 'модель · дома' };
        }
        case 'own':
            return { tone: 'own', label: 'навык шлюза' };
        case 'unsupported':
            return { tone: 'off', label: 'пока не умеем' };
        default:
            return request.kind === 'say' ? { tone: 'own', label: 'шлюз' } : undefined;
    }
}

/** Итог запроса пилюлей: класс и слово; идущий — его шаг. */
export function statusOf(request: RequestView): { cls: string; label: string } {
    switch (request.status) {
        case 'active':
            return { cls: 'pill-own req-going', label: request.phase ? PHASE_WORDS[request.phase] : 'идёт' };
        case 'answered':
            return { cls: 'pill-off', label: request.ms !== undefined ? seconds(request.ms) : 'ответ' };
        case 'silent':
            return { cls: 'pill-off', label: 'тишина' };
        case 'cancelled':
            return { cls: 'pill-off', label: 'прервано' };
        default:
            return { cls: 'pill-error', label: 'ошибка' };
    }
}
