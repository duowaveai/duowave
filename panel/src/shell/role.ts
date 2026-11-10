// Роль пульта (docs/plans/desk.md): «Шлюз» — пульт того, кто отвечает колонке (обычно самой колонки), «Помощник» —
// DuoWave на компьютере, когда шлюз работает на колонке. Роль — не тема: пульт знает её сам, по /health, и показывает
// рамкой (верхняя строка, док, боковая панель): у шлюза белая, у помощника тёмная — `<html data-role>`.

import type { Health, PlacedStation } from '../api';

export type Role = 'gateway' | 'helper';

export interface RoleInfo {
    role: Role;
    /** Колонка помощника: её шлюз работает на ней самой. */
    station?: PlacedStation;
    /** Шлюз на колонке и компьютер помогает ей (слух, голос, модель): размещение «колонка + компьютер». */
    helping: boolean;
    /** У шлюза на колонке: помощник есть и на связи ли он. */
    helper?: { state: 'up' | 'down' | 'none'; enabled: boolean };
}

/**
 * Кто этот пульт. Компьютер, у выбранной колонки которого шлюз на ней самой, — помощник; всё остальное — шлюз:
 * колонка со своим шлюзом, компьютер, который отвечает колонке сам, и шлюз постарше без размещения.
 */
export function roleOf(health: Health | null, device?: string | null): RoleInfo {
    const placement = health?.placement;
    if (!placement) return { role: 'gateway', helping: false };
    if (placement.role === 'station') {
        const home = placement.homeServer;
        return {
            role: 'gateway',
            helping: placement.placement === 'stationServer',
            helper: home && home.url ? { state: home.state, enabled: home.enabled !== false } : undefined,
        };
    }
    const placed = (device ? placement.stations?.find((entry) => entry.device === device) : undefined) ?? placement.station;
    if (placed && placed.placement !== 'pc') return { role: 'helper', station: placed, helping: placed.placement === 'stationServer' };
    return { role: 'gateway', helping: false };
}

/** Имя, с которым колонка приходит в первый раз (screens/stations.tsx, DEFAULT_NAME): своего имени у неё ещё нет. */
const FIRST_NAME = 'Колонка';

/**
 * Имя колонки, о которой этот пульт, когда его знает шлюз: у помощника — из записи компьютера, у шлюза на колонке —
 * из её /health (перенос приносит имя из записи компьютера). Иначе — undefined, и пульт берёт выбранную колонку:
 * выбор хранится в браузере, общий у страницы помощника и окна колонки, и мог указывать не на неё.
 */
export function ownName(health: Health | null | undefined, role: RoleInfo): string | undefined {
    const name = role.role === 'helper' ? role.station?.name : health?.placement?.role === 'station' ? health.placement.name : undefined;
    return name && name !== FIRST_NAME ? name : undefined;
}

/** Роль на странице: стили рамки читают `<html data-role>`. */
export function applyRole(role: Role): void {
    const root = document.documentElement;
    if (root.dataset.role !== role) root.dataset.role = role;
}
