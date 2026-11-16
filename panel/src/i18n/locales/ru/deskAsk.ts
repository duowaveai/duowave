// Тексты строки «Поставить на стол?» под сценой (screens/desk-ask.tsx, docs/plans/start-desk.md, этап 5).
// Русский — исходный словарь; английский — тот же набор ключей.

export default {
    asked: (title: string, times: number) => `Вы ${times === 2 ? 'дважды' : 'не раз'} спрашивали про «${title}». Поставить на стол?`,
    put: 'Поставить',
    no: 'Не надо',
};
