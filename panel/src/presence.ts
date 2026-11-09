import { StationPresence } from './api';
import { timeAgo } from './time';

// Как колонку слышно — для шапки и окна «Колонки». Шлюз узнаёт это без единого запроса к колонке: по её
// голосовому каналу и её собственным клиентам (мост микрофона, опрос звонков), которые поднимаются за секунды.

/**
 * online — голосовой канал открыт; waiting — колонка на месте (её клиенты на связи), голосовой канал откроется
 * сам или с первым «Алиса»; reconnecting — колонка между сокетами и откроет следующий сама: её сокет только что
 * закрылся штатно (смена режима) или шлюз только что перезапустился, а колонка была до него; offline — её не
 * слышно или её сокет оборвался; unknown — пульт ещё не знает.
 */
export type Reach = 'online' | 'waiting' | 'reconnecting' | 'offline' | 'unknown';

/** Из присутствия (живой канал), а без него — по голосовой сессии; undefined — ещё ничего не прочитано. */
export function reachOf(presence: StationPresence | undefined, voiceSession: boolean | undefined): Reach {
    if (presence) {
        if (presence.state === 'voice') return 'online';
        if (presence.state === 'alive') return 'waiting';
        return presence.state;
    }
    if (voiceSession === undefined) return 'unknown';
    return voiceSession ? 'online' : 'offline';
}

/** На месте — для дашборда и счётчика «в сети N из M». */
export const isReachable = (reach: Reach): boolean => reach === 'online' || reach === 'waiting';

/** Класс точки состояния: полная — на связи, кольцо — голос ждёт, мигающая — переподключается. */
export function dotClass(reach: Reach): string {
    return `status-dot dot-${reach}`;
}

/** Строка под именем колонки в шапке. */
export function reachLine(reach: Reach, modeWord: string, lastSeen?: string): string {
    switch (reach) {
        case 'online':
            return `в сети · ${modeWord}`;
        case 'waiting':
            return 'в сети · голос ждёт «Алиса»';
        case 'reconnecting':
            return 'переподключается…';
        case 'unknown':
            return 'проверяю связь…';
        default:
            return lastSeen ? `не в сети · была ${timeAgo(lastSeen)}` : 'не в сети';
    }
}
