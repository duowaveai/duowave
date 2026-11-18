// Роль пульта (docs/plans/desk.md): «Шлюз» — пульт центра дома (обычно самой колонки), «Помощник» — DuoWave на
// компьютере-Турбо, когда центр работает на колонке: его окно уходит к центру (`?via=`, app.tsx). Роль пульт знает
// сам, по /health. Рамкой она больше не видна: пульт один (docs/plans/home-and-devices.md, этап 1).

import type { Away, HelperBrief, Health, PlacedStation, PlacementHealth } from '../api';

export type Role = 'gateway' | 'helper';

export interface RoleInfo {
    role: Role;
    /** Колонка помощника: её шлюз работает на ней самой. */
    station?: PlacedStation;
    /** Шлюз на колонке и компьютер помогает ей (слух, голос, модель): размещение «колонка + компьютер». */
    helping: boolean;
    /**
     * У шлюза на колонке: помощник есть и на связи ли он; `kept` — включённых не пускает регулятор «Что может уходить
     * из дома» (это выбор, не поломка); `name` — кто (шлюз 0.11 имени не знает).
     */
    helper?: { state: 'up' | 'down' | 'none' | 'kept'; enabled: boolean; name?: string };
}

/**
 * Не пускает ли его регулятор. Шлюз говорит это сам (`allowed`); /health этапа 3 поля не присылает — тогда по месту:
 * «Только дома» не пускает «мой сервер» и «облако»; «Дом и моё облако» и «Любые облака» пускают любого помощника —
 * дальше «моего» помощник не бывает.
 */
const keptOut = (entry: HelperBrief, away: Away): boolean =>
    entry.allowed === false || (entry.allowed === undefined && away === 'home' && entry.kind !== 'home');

/**
 * Помощник шлюза на колонке для строки состояния (docs/plans/home-and-helpers.md, блок 1): по списку помощников —
 * первый на связи из включённых, которых пускает регулятор, иначе первый из них; все выключены — `none`; всех
 * включённых не пускает регулятор — `kept`. Шлюз 0.11 списка не присылает — тогда по его домашнему серверу, как раньше.
 */
export function helperOf(placement: PlacementHealth): RoleInfo['helper'] {
    const helpers = placement.helpers;
    if (helpers) {
        const others = helpers.list.filter((entry) => !entry.builtin);
        if (others.length === 0) return undefined;
        const on = others.filter((entry) => entry.enabled);
        if (on.length === 0) return { state: 'none', enabled: false, name: others[0].name };
        const allowed = on.filter((entry) => !keptOut(entry, helpers.away));
        if (allowed.length === 0) return { state: 'kept', enabled: true, name: on[0].name };
        const up = allowed.find((entry) => entry.state === 'up');
        return { state: up ? 'up' : 'down', enabled: true, name: (up ?? allowed[0]).name };
    }
    const home = placement.homeServer;
    return home && home.url ? { state: home.state, enabled: home.enabled !== false } : undefined;
}

/**
 * Кто этот пульт. Компьютер, у выбранной колонки которого шлюз на ней самой, — помощник; всё остальное — шлюз:
 * колонка со своим шлюзом, компьютер, который отвечает колонке сам, и шлюз постарше без размещения.
 */
export function roleOf(health: Health | null, device?: string | null): RoleInfo {
    const placement = health?.placement;
    if (!placement) return { role: 'gateway', helping: false };
    if (placement.role === 'station') return { role: 'gateway', helping: placement.placement === 'stationServer', helper: helperOf(placement) };
    const placed = (device ? placement.stations?.find((entry) => entry.device === device) : undefined) ?? placement.station;
    if (placed && placed.placement !== 'pc') return { role: 'helper', station: placed, helping: placed.placement === 'stationServer' };
    return { role: 'gateway', helping: false };
}

/**
 * Помогает ли этот компьютер кому-то (docs/plans/home-and-helpers.md, этап 8): колонке, которой включено «Помогать»
 * (`stationServer`), или шлюзу, подключённому ключом (`linked` — сколько их в «Помогаю»). Колонка, что «работает сама»,
 * на компьютер не опирается. Пока помогает — на «Этом компьютере» стоит «Не давать компьютеру спать».
 */
export function helpsSomeone(stations: Pick<PlacedStation, 'placement'>[], linked: number): boolean {
    return linked > 0 || stations.some((entry) => entry.placement === 'stationServer');
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
