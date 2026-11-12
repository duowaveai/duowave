/** Каталог способностей — заголовки и иконки те же, что в gateway/src/skills/capabilities.rs гейтвея. */

export interface CapabilityMeta {
    name: string;
    title: string;
    icon: 'chat' | 'speaker' | 'player' | 'music' | 'timer' | 'home' | 'phone' | 'moon' | 'user';
}

export const CAPABILITIES: CapabilityMeta[] = [
    { name: 'chat', title: 'Разговор, время, дата', icon: 'chat' },
    { name: 'volume', title: 'Громкость', icon: 'speaker' },
    { name: 'playback', title: 'Стоп, пауза, дальше', icon: 'player' },
    { name: 'radio', title: 'Радио', icon: 'music' },
    { name: 'music', title: 'Музыка', icon: 'music' },
    { name: 'tales', title: 'Сказки, аудиокниги', icon: 'music' },
    { name: 'games', title: 'Игры', icon: 'chat' },
    { name: 'skills', title: 'Навыки Диалогов', icon: 'chat' },
    { name: 'timers', title: 'Таймеры', icon: 'timer' },
    { name: 'alarms', title: 'Будильники', icon: 'timer' },
    { name: 'reminders', title: 'Напоминания', icon: 'timer' },
    { name: 'later', title: 'Отложенные команды', icon: 'timer' },
    { name: 'weather', title: 'Погода', icon: 'chat' },
    { name: 'news', title: 'Новости', icon: 'chat' },
    { name: 'morning_show', title: 'Утреннее шоу', icon: 'player' },
    { name: 'smart_home', title: 'Умный дом', icon: 'home' },
    { name: 'bluetooth', title: 'Bluetooth', icon: 'speaker' },
    { name: 'led', title: 'Подсветка', icon: 'speaker' },
    { name: 'do_not_disturb', title: 'Не беспокоить', icon: 'moon' },
    { name: 'calls', title: 'Звонки и сообщения', icon: 'phone' },
    { name: 'music_recognition', title: 'Что играет', icon: 'music' },
    { name: 'account', title: 'Сервисы аккаунта', icon: 'user' },
    { name: 'plugins', title: 'Приложения', icon: 'chat' },
];

export const CAPABILITY_TITLES: Record<string, string> = Object.fromEntries(
    CAPABILITIES.map((c) => [c.name, c.title]),
);

export function capabilityIcon(name: string): CapabilityMeta['icon'] {
    return CAPABILITIES.find((c) => c.name === name)?.icon ?? 'chat';
}
