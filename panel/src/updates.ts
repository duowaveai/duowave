import { useEffect, useState } from 'preact/hooks';

import { UpdateStatus, api, can } from './api';
import { BannerOffer, Offered, bannerOffer } from './update-offer';
import { VIA } from './via';

export type { BannerOffer, Offered } from './update-offer';

// Что шлюз знает о новой версии — одно на весь пульт: точка на «Дом» и «Шлюз», строка на главном экране и раздел
// «Обновления» читают и пишут одно и то же. Видят это только те, кому можно ставить обновления (system.update).
// В окне колонки (`?via=`) таких знаний два: шлюза пульта (колонки-центра, через проход) и самого компьютера (мимо
// прохода, api.ownUpdates) — компьютер обновляется первым и отдаёт новую версию колонке.

/** Раздел «Обновления» сам спрашивает ленту, если с прошлой проверки прошло больше этого. */
export const FRESH_MS = 10 * 60_000;

/** Как часто окно колонки перечитывает, что знает о новой версии сам компьютер: живой канал окна колонки несёт
 * только её события, о находке компьютера он молчит. Это вопрос своему шлюзу, без похода в ленту. */
export const OWN_READ_MS = 5 * 60_000;

type Listener = (status: UpdateStatus | null) => void;

function store() {
    let current: UpdateStatus | null = null;
    const listeners = new Set<Listener>();
    return {
        set(status: UpdateStatus | null): void {
            current = status;
            listeners.forEach((listener) => listener(status));
        },
        use(): UpdateStatus | null {
            const [status, setStatus] = useState(current);
            useEffect(() => {
                listeners.add(setStatus);
                setStatus(current);
                return () => void listeners.delete(setStatus);
            }, []);
            return status;
        },
    };
}

const pult = store();
const own = store();

export function setUpdateStatus(status: UpdateStatus | null): void {
    pult.set(status);
}

/** Что знает о новой версии сам компьютер, в окне колонки (раздел «Обновления» → «Этот компьютер»). */
export function setOwnUpdateStatus(status: UpdateStatus | null): void {
    own.set(status);
}

/** Перечитать, что знает шлюз (без похода в ленту). */
export function readUpdateStatus(): Promise<void> {
    if (!can('system.update')) return Promise.resolve();
    return api
        .updates()
        .then(setUpdateStatus)
        .catch(() => {});
}

/** Перечитать, что знает о новой версии сам компьютер; только в окне колонки — иначе это тот же шлюз пульта. */
export function readOwnUpdateStatus(): Promise<void> {
    if (!VIA || !can('system.update')) return Promise.resolve();
    return api
        .ownUpdates()
        .then(setOwnUpdateStatus)
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
export function offeredUpdate(status: UpdateStatus | null): Offered | null {
    const version = offeredVersion(status);
    if (!version || !status?.available) return null;
    return { version, critical: !!status.available.critical, auto: !!status.autoCritical };
}

/** Есть ли что поставить — у шлюза пульта или, в окне колонки, у самого компьютера: для точек на «Дом» и «Шлюз». */
export function useOfferedVersion(): string | null {
    // Оба хука — всегда, в одном порядке: `??` пропустил бы второй.
    const [hub, computer] = [pult.use(), own.use()];
    return offeredVersion(hub) ?? offeredVersion(computer);
}

/** Строка на главном экране: версия шлюза пульта или самого компьютера (update-offer.ts — какая из двух). */
export function useOfferedUpdate(): BannerOffer | null {
    const [hub, computer] = [pult.use(), own.use()];
    return bannerOffer(offeredUpdate(hub), offeredUpdate(computer));
}
