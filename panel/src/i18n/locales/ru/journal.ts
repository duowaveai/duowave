// Тексты: screens/journal.tsx.

export default {
    keepNone: 'Не хранить',
    keepWeek: 'Неделю',
    keepMonth: 'Месяц',
    keepYear: 'Год',
    keepDays: (days: number) => `${days} дн.`,
    noSessions: 'Шлюз не отдал сессии — попробуйте позже.',

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
};
