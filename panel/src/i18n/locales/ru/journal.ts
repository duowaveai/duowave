// Тексты: screens/journal.tsx.
import { plural } from '../../format';

export default {
    keepNone: 'Не хранить',
    keepWeek: 'Неделю',
    keepMonth: 'Месяц',
    keepYear: 'Год',
    keepDays: (days: number) => `${days} дн.`,
    noSessions: 'Хаб не отдал сессии — попробуйте позже.',

    stationTitle: 'Колонка',
    disconnected: (ago: string) => `отключилась ${ago}`,
    connected: (ago: string) => `подключена ${ago}`,
    firmware: 'прошивка',
    events: 'событий',
    canSpeak: 'умеет говорить',
    yandexOn: 'Яндекс на связи',
    yandexOff: 'Яндекса нет',

    noWords: 'Без слов',
    quote: (text: string) => `«${text}»`,
    quotePartial: (text: string) => `«${text}…»`,
    playAt: (time: string) => ` на ${time}`,
    noSteps: 'Шагов нет: запрос закончился, не начавшись.',

    keepLabel: 'Хранить журнал',
    keepAria: 'Сколько хранить журнал запросов',
    deleteAllNow: 'Весь журнал запросов удалится сейчас.',
    deleteOlderNow: (days: number) => `Запросы старше ${days} дн. удалятся сейчас.`,
    delete: 'Удалить',
    cancel: 'Отмена',

    requests: 'Запросы',
    notKept: 'не хранятся',
    keptFor: (days: number) => `хранятся ${days} дн.`,
    emptyStation: 'Пока пусто: спросите колонку — запрос появится здесь, пока она отвечает.',
    emptyComputer: 'Пока пусто: нажмите «Сказать» и спросите — запрос появится здесь, пока DuoWave отвечает.',
    loading: 'Загружаю…',
    more: 'Ещё',

    noRightStation: 'Что говорили колонке, видит тот, кому администратор дал право на журнал.',
    noRightComputer: 'Что говорили DuoWave, видит тот, кому администратор дал право на журнал.',
    unsupported: 'Пока не умеем',
    unsupportedEmpty: 'Пусто: всё, что просили, кто-то обслужил.',
    opened: (activator: 'word' | 'button' | 'answer' | 'text', name?: string) =>
        activator === 'word' ? `словом «${name ?? '…'}»` : activator === 'button' ? 'по кнопке' : activator === 'answer' ? 'ответ на вопрос' : 'текстом',
    firstWord: (time: string) => `первое слово через ${time}`,
    tokens: (input: number, output: number, cached: number, rounds: number) =>
        `токены ${input} → ${output}${cached ? `, из кэша ${cached}` : ''}${rounds > 1 ? `, кругов ${rounds}` : ''}`,
    /** Сделка ответа: место, модель и доллары (docs/plans/router-and-works.md, «Устройство: пульт»). */
    deal: (seat: string, model: string, money: string) => [seat, model, money].filter(Boolean).join(' · '),

    // Блок «Мышление» в подробностях запроса (docs/plans/thinking.md, «The turn's line»).
    thinkingTitle: 'Мышление',
    /** `tokens` — число коротко («1,9 тыс.»), `n` — оно же целиком, для слова. */
    thinkingCard: (tokens: string, n: number) =>
        `карточка ${tokens} ${n >= 1000 ? 'токенов' : plural(n, { one: 'токен', few: 'токена', many: 'токенов' })}`,
    thinkingFast: (step: string) => `быстрый путь: ${step}`,
    thinkingOpened: (groups: string) => `открыто: ${groups}`,
    thinkingRounds: (n: number) => `${n} ${plural(n, { one: 'круг', few: 'круга', many: 'кругов' })}`,
    thinkingFirstWord: (time: string) => `первое слово ${time}`,
    /** Карточка в тени: хватило бы её, не хватило бы или инструменты не понадобились. */
    thinkingShadow: {
        hit: 'карточка в тени: хватило бы',
        miss: 'карточка в тени: не хватило бы',
        none: 'карточка в тени: инструменты не понадобились',
    },
    thinkingWaits: (n: number) => `ждали: ${n}`,
    /** Что человек сделал потом. Другая фраза или тишина — ничего не пишем. */
    thinkingNext: {
        cut: 'перебили',
        repeated: 'повторили',
        not_that: 'не то',
        answered: 'ответили на вопрос',
    },
    /** Встроенные группы инструментов. Группы приложений и MCP (`app:…`, `mcp:…`) — как их назвал шлюз. */
    thinkingGroups: {
        weather: 'погода',
        news: 'новости',
        calendar: 'календарь',
        timers: 'таймеры',
        alarms: 'будильники',
        memory: 'память',
        works: 'работы',
        music: 'музыка',
        radio: 'радио',
        sound: 'звук',
    },
};
