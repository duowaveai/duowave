import { locale, tr } from './i18n';

/** Время по-человечески: «только что», «7 мин назад», «14:32», «вчера 23:04». */

export function clock(iso: string): string {
    return new Date(iso).toLocaleTimeString(locale(), { hour: '2-digit', minute: '2-digit' });
}

/**
 * С какого времени: «14:02» сегодня, «вчера 14:02», иначе «12.11, 14:02». `from` — после «с»: «с 14:02 вчера» (по-русски
 * не «с вчера 14:02»).
 */
export function since(iso: string, from = false): string {
    const date = new Date(iso);
    const at = clock(iso);
    if (date.toDateString() === new Date().toDateString()) return at;
    if (date.toDateString() === new Date(Date.now() - 86_400_000).toDateString()) return (from ? tr.core.fromYesterday : tr.core.yesterdayAt)(at);
    return date.toLocaleString(locale(), { day: '2-digit', month: '2-digit', hour: '2-digit', minute: '2-digit' });
}

export function timeAgo(iso: string): string {
    const ms = Date.now() - new Date(iso).getTime();
    if (ms < 45_000) return tr.core.justNow;
    const minutes = Math.round(ms / 60_000);
    if (minutes < 60) return tr.core.minutesAgo(minutes);
    const at = clock(iso);
    const today = new Date().toDateString() === new Date(iso).toDateString();
    if (today) return at;
    const yesterday = new Date(Date.now() - 86_400_000);
    if (yesterday.toDateString() === new Date(iso).toDateString()) return tr.core.yesterdayAt(at);
    return new Date(iso).toLocaleDateString(locale(), { day: 'numeric', month: 'short' });
}
