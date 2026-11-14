import { tr } from './i18n';

/** Каталог способностей — заголовки и иконки те же, что в gateway/src/skills/capabilities.rs гейтвея. */

export interface CapabilityMeta {
    name: string;
    title: string;
    icon: 'chat' | 'speaker' | 'player' | 'music' | 'timer' | 'home' | 'phone' | 'moon' | 'user';
}

export const CAPABILITIES: CapabilityMeta[] = [
    { name: 'chat', get title() { return tr.core.cap.chat; }, icon: 'chat' },
    { name: 'volume', get title() { return tr.core.cap.volume; }, icon: 'speaker' },
    { name: 'playback', get title() { return tr.core.cap.playback; }, icon: 'player' },
    { name: 'radio', get title() { return tr.core.cap.radio; }, icon: 'music' },
    { name: 'music', get title() { return tr.core.cap.music; }, icon: 'music' },
    { name: 'tales', get title() { return tr.core.cap.tales; }, icon: 'music' },
    { name: 'games', get title() { return tr.core.cap.games; }, icon: 'chat' },
    { name: 'skills', get title() { return tr.core.cap.skills; }, icon: 'chat' },
    { name: 'timers', get title() { return tr.core.cap.timers; }, icon: 'timer' },
    { name: 'alarms', get title() { return tr.core.cap.alarms; }, icon: 'timer' },
    { name: 'reminders', get title() { return tr.core.cap.reminders; }, icon: 'timer' },
    { name: 'later', get title() { return tr.core.cap.later; }, icon: 'timer' },
    { name: 'weather', get title() { return tr.core.cap.weather; }, icon: 'chat' },
    { name: 'news', get title() { return tr.core.cap.news; }, icon: 'chat' },
    { name: 'morning_show', get title() { return tr.core.cap.morningShow; }, icon: 'player' },
    { name: 'smart_home', get title() { return tr.core.cap.smartHome; }, icon: 'home' },
    { name: 'bluetooth', get title() { return tr.core.cap.bluetooth; }, icon: 'speaker' },
    { name: 'led', get title() { return tr.core.cap.led; }, icon: 'speaker' },
    { name: 'do_not_disturb', get title() { return tr.core.cap.doNotDisturb; }, icon: 'moon' },
    { name: 'calls', get title() { return tr.core.cap.calls; }, icon: 'phone' },
    { name: 'music_recognition', get title() { return tr.core.cap.musicRecognition; }, icon: 'music' },
    { name: 'account', get title() { return tr.core.cap.account; }, icon: 'user' },
    { name: 'plugins', get title() { return tr.core.cap.plugins; }, icon: 'chat' },
];

/** Названия по коду: геттеры — читают язык при обращении, а не при загрузке модуля. */
export const CAPABILITY_TITLES: Record<string, string> = Object.defineProperties(
    {},
    Object.fromEntries(CAPABILITIES.map((c) => [c.name, { enumerable: true, get: () => c.title }])),
);

export function capabilityIcon(name: string): CapabilityMeta['icon'] {
    return CAPABILITIES.find((c) => c.name === name)?.icon ?? 'chat';
}
