// Тексты: frame/* — окошко приложения и его мост.

export default {
    appCrashedLog: 'Окошко: приложение упало',
    appCrashed: 'Окошко приложения упало:',
    moduleNotHere: 'модуль не с этого шлюза',
    noPlugin: 'модуль не отдаёт плагин: export default definePlugin({…})',
    noPart: (part: string) => `у приложения нет «${part}»`,
    failed: 'не вышло',
    noTask: 'нет задачи',
    midiTooBig: 'Файл больше 256 КБ — это не простая мелодия.',
    probeSilent: (id: string) => `окошко приложения ${id} не ответило`,
    frameTitle: (id: string) => `Окошко приложения ${id}`,
    reloadedItself: 'окошко перезагрузилось само — закрыто',
    notAFile: 'не файл',
    asks: (id: string, title: string) => `«${id}» спрашивает: ${title}`,
    didNotOpen: 'Окошко не открылось:',
};
