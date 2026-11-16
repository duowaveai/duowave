// Одно имя колонки (владелец 16.11): у колонки было три имени — запись в списке компьютера («Спальня»), запись колонки
// о самой себе («Колонка» → по модели «Лайт 2») и имя её шлюза («Колонка Лайт 2»). Главное — имя из списка
// компьютера: окно колонки на компьютере (`?via=`) показывает его, а переименование в любом окне пишет имя в обе
// записи. Колонка постарше (0.13) своего имени шлюза не знает — поэтому это делает пульт, а не шлюз.

import { useEffect, useState } from 'preact/hooks';

import { ownCall, stationCall } from './api';
import type { Health, PlacedStation } from './api';
import { TOP_VIA, VIA } from './via';

/** Имя новой колонки: им не называют, его заменяют (gateway DEFAULT_STATION_NAME). */
const FIRST_NAME = 'Колонка';

let homeName: string | null | undefined;
const listeners = new Set<() => void>();

async function readHomeName(): Promise<void> {
    try {
        const health = await ownCall<Health>('GET', '/health');
        const placed = health?.placement?.stations ?? (health?.placement?.station ? [health.placement.station] : []);
        const name = placed.find((entry) => entry.key === VIA)?.name;
        homeName = name && name.trim() && name !== FIRST_NAME ? name : null;
    } catch {
        homeName = null;
    }
    listeners.forEach((listener) => listener());
}

/** В окне колонки на компьютере — имя колонки из списка компьютера; иначе (и пока не прочитано) — undefined. */
export function useHomeStationName(): string | undefined {
    const [, bump] = useState(0);
    useEffect(() => {
        if (!TOP_VIA) return undefined;
        const listener = () => bump((n) => n + 1);
        listeners.add(listener);
        if (homeName === undefined) void readHomeName();
        return () => void listeners.delete(listener);
    }, []);
    return TOP_VIA ? (homeName ?? undefined) : undefined;
}

/**
 * Колонку переименовали в этом окне — то же имя второй записи: из окна колонки — записи компьютера о ней, из пульта
 * компьютера — записи колонки о себе (через проход; колонка не на связи — не беда, имя главное у компьютера).
 */
export async function syncStationName(device: string, name: string, health: Health | null | undefined): Promise<void> {
    const clean = name.trim();
    if (!clean) return;
    if (TOP_VIA) {
        await ownCall('PATCH', `/api/v1/stations/${encodeURIComponent(device)}`, { name: clean }).catch(() => undefined);
        homeName = clean;
        listeners.forEach((listener) => listener());
        return;
    }
    const placed: PlacedStation[] = health?.placement?.stations ?? (health?.placement?.station ? [health.placement.station] : []);
    const station = placed.find((entry) => entry.device === device && entry.placement !== 'pc');
    if (station) await stationCall(station.key, 'PATCH', `/api/v1/stations/${encodeURIComponent(device)}`, { name: clean }).catch(() => undefined);
}

