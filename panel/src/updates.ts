import { useEffect, useState } from 'preact/hooks';

import { UpdateStatus, api, can } from './api';

// Что шлюз знает о новой версии — одно на весь пульт: точка на «Дом» и «Шлюз», строка на главном экране и раздел
// «Обновления» читают и пишут одно и то же. Видят это только те, кому можно ставить обновления (system.update).

/** Раздел «Обновления» сам спрашивает ленту, если с прошлой проверки прошло больше этого. */
export const FRESH_MS = 10 * 60_000;

let current: UpdateStatus | null = null;
const listeners = new Set<(status: UpdateStatus | null) => void>();

export function setUpdateStatus(status: UpdateStatus | null): void {
    current = status;
    listeners.forEach((listener) => listener(status));
}

/** Перечитать, что знает шлюз (без похода в ленту). */
export function readUpdateStatus(): Promise<void> {
    if (!can('system.update')) return Promise.resolve();
    return api
        .updates()
        .then(setUpdateStatus)
        .catch(() => {});
}

/** Проверка ленты нужна: её давно не было или не было вовсе. */
export function checkIsStale(status: UpdateStatus, now = Date.now()): boolean {
    return status.feed && (!status.checkedAt || now - Date.parse(status.checkedAt) > FRESH_MS);
}

/** Версия, которую стоит поставить: предложена и ещё не ставится; null, если нет или смотреть не положено. */
export function offeredVersion(status: UpdateStatus | null): string | null {
    if (!status?.available || status.installing || !can('system.update')) return null;
    return status.available.version;
}

/** Предложенная версия для строки на главном экране: срочная ли она и поставит ли её шлюз сам. */
export interface Offered {
    version: string;
    critical: boolean;
    auto: boolean;
}

export function offeredUpdate(status: UpdateStatus | null): Offered | null {
    const version = offeredVersion(status);
    if (!version || !status?.available) return null;
    return { version, critical: !!status.available.critical, auto: !!status.autoCritical };
}

function useUpdateStatus(): UpdateStatus | null {
    const [status, setStatus] = useState(current);
    useEffect(() => {
        listeners.add(setStatus);
        setStatus(current);
        return () => void listeners.delete(setStatus);
    }, []);
    return status;
}

export function useOfferedVersion(): string | null {
    return offeredVersion(useUpdateStatus());
}

export function useOfferedUpdate(): Offered | null {
    return offeredUpdate(useUpdateStatus());
}
