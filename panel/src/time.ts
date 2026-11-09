/** Время по-человечески: «только что», «7 мин назад», «14:32», «вчера 23:04». */

export function clock(iso: string): string {
    return new Date(iso).toLocaleTimeString('ru-RU', { hour: '2-digit', minute: '2-digit' });
}

export function timeAgo(iso: string): string {
    const ms = Date.now() - new Date(iso).getTime();
    if (ms < 45_000) return 'только что';
    const minutes = Math.round(ms / 60_000);
    if (minutes < 60) return `${minutes} мин назад`;
    const at = clock(iso);
    const today = new Date().toDateString() === new Date(iso).toDateString();
    if (today) return at;
    const yesterday = new Date(Date.now() - 86_400_000);
    if (yesterday.toDateString() === new Date(iso).toDateString()) return `вчера ${at}`;
    return new Date(iso).toLocaleDateString('ru-RU', { day: 'numeric', month: 'short' });
}
