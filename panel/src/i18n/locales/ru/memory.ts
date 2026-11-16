// Тексты: screens/memory.tsx.

export default {
    pressureNormal: 'давление в норме',
    pressureWarn: 'ОЗУ мало',
    pressureCritical: 'ОЗУ очень мало',
    segSystem: 'Система и Алиса',
    segGateway: 'DuoWave',
    segPlugins: 'Приложения',
    segFree: 'Свободно',
    title: 'Ресурсы колонки',
    mb: (n: number) => `${n} МБ`,
    budget: (budget: number, reserve: number) =>
        `Бюджет ОЗУ для приложений — ${budget} МБ. Приложение занимает в нём свой объём, только пока загружено; простаивающие выгружаются и освобождают ОЗУ. Запас для системы и Алисы — ${reserve} МБ.`,
    actionWarn: ' Под сильным давлением приложения не выключаются — только предупреждение.',
    actionOff: ' Под сильным давлением последнее включённое из загруженных приложений выключится само.',
    disk: (free: number, packages: number, reserve: number) =>
        `Память: свободно ${free} МБ, приложения занимают ${packages} МБ, ${reserve} МБ — запас на обновление шлюза.`,
    pictures: (used: string, limit: string | null) => `Картинки приложений: ${used} МБ${limit ? ` из ${limit}` : ''}.`,
    lineMemory: (used: string, budget: number) => `ОЗУ ${used} из ${budget} МБ`,
    lineBudget: (budget: number) => `бюджет ОЗУ ${budget} МБ`,
    lineCalls: (n: number) => `вызовов ${n}`,
    lineErrors: (n: number) => `ошибок ${n}`,
};
