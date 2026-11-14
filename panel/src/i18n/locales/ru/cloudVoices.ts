// Тексты: screens/voice-providers.tsx — «Облачные голоса» во вкладке «Голос»: поставщики голосов для приложений.

export default {
    title: 'Облачные голоса',
    forApps: 'для приложений',
    intro: 'Приложения озвучивают реплики голосами компьютера, а с ключом — и облачными. Текст реплик уходит поставщику и тратит ваш тариф, поэтому каждое приложение сначала спрашивает ваше «да».',
    keySet: (hint: string) => `ключ задан · …${hint}`,
    noKey: 'ключа нет',
    cloud: 'облако',
    paid: 'платно',
    connected: 'подключён',
    check: 'проверить',
    works: (voices: number) => `Работает: голосов — ${voices}`,
    left: (left: string, limit: string, unit: string, tier: string | null) =>
        `Осталось ${left} ${unit} из ${limit}${tier ? ` · тариф ${tier}` : ''}`,
    newKey: 'Новый ключ',
    keyOf: (title: string) => `Ключ ${title}`,
    save: 'сохранить',
    removeKey: 'убрать ключ',
    whereKey: 'Где взять ключ',
    keyByPeople: 'Ключ вставляет человек с правом подключений.',
    removeTitle: (title: string) => `Убрать ключ ${title}?`,
    removeText: 'Приложения перестанут говорить его голосами.',
    remove: 'Убрать',
    removed: 'Ключ убран.',
};
