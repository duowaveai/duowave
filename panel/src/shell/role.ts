// Роль пульта (docs/plans/desk.md): «Шлюз» — пульт того, кто отвечает колонке (обычно самой колонки), «Помощник» —
// DuoWave на компьютере, когда шлюз работает на колонке. Роль — не тема: пульт знает её сам, по /health, и показывает
// рамкой (верхняя строка, док, боковая панель): у шлюза белая, у помощника тёмная — `<html data-role>`.

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

/**
 * «Где я» (решение владельца 16.11): вкладки устройств дома в верхней строке — нажатая вкладка — чей это пульт. Колонка
 * со своим шлюзом — «шлюз», компьютер, что ей помогает, — «Турбо». Переход — одно нажатие: в окне компьютера пульт
 * колонки — `?via=<колонка>`, свой пульт компьютера — `?local`. Чужого пульта у колонки, открытой браузером, нет: её
 * Турбо — вкладкой без перехода (адреса его пульта она не знает).
 */
export interface DeviceTab {
    key: string;
    /** Что за устройство: рисунок вкладки. */
    kind: 'station' | 'computer' | 'server';
    /** Имя колонки или Турбо; у этого компьютера имени нет — «Этот компьютер». */
    name?: string;
    /** Кто он в доме: шлюз, Турбо, шлюз со встроенным Турбо; `asleep` — Турбо не на связи. */
    tag?: 'gateway' | 'turbo' | 'gatewayTurbo' | 'asleep';
    /** Чей это пульт. */
    on: boolean;
    /** Куда ведёт нажатие (тот же источник); нет — вкладка только показывает. */
    href?: string;
    /** Устройства сейчас не слышно. */
    off?: boolean;
    /** Нажатая вкладка открывает выбор колонки (у шлюза на компьютере с колонками). */
    pick?: boolean;
}

export interface WhereAmI {
    tabs: DeviceTab[];
    /** «Вы в пульте колонки / компьютера»; не знаем — строки нет. */
    here?: 'station' | 'computer';
}

/** Что знает пульт, чтобы нарисовать вкладки: размещение, роль, адрес страницы и выбранная колонка. */
export interface WhereInput {
    placement?: PlacementHealth;
    role: RoleInfo;
    /** Колонка, чей пульт открыт через проход компьютера (`?via=`), во всё окно. */
    via?: string | null;
    /** Режим «Микрофон»: колонки в пульте нет. */
    computer: boolean;
    /** Адрес страницы без поиска (`/panel/`) и место в пульте (`#/…`): вкладка ведёт туда же. */
    path: string;
    hash: string;
    /** Имя колонки, о которой этот пульт (ownName или выбранная): у нажатой вкладки колонки. */
    current?: string;
}

/** Из «Дома» в «Дом»: страница настроек «Дом» есть у обоих пультов; иначе — на стол. */
const carried = (hash: string): string => (/^#\/settings\/(helpers|helper|placement)\b/.test(hash) ? '#/settings/helpers' : '');

/** Турбо шлюза вкладками: включённые, кроме встроенного (он — сам этот компьютер), не больше двух. */
function turboTabs(placement: PlacementHealth | undefined): DeviceTab[] {
    const list = (placement?.helpers?.list ?? []).filter((entry) => !entry.builtin && entry.enabled);
    return list.slice(0, 2).map((entry) => ({
        key: `turbo:${entry.id}`,
        kind: entry.kind === 'home' ? ('computer' as const) : ('server' as const),
        name: entry.name,
        tag: entry.state === 'up' ? ('turbo' as const) : ('asleep' as const),
        on: false,
        off: entry.state !== 'up',
    }));
}

export function whereAmI(input: WhereInput): WhereAmI {
    const { placement, role, via, computer, path } = input;
    const hash = carried(input.hash);
    const named = (name: string | undefined): string | undefined => (name && name !== FIRST_NAME ? name : undefined);
    // Пульт колонки в окне компьютера: колонка — шлюз, рядом — свой пульт компьютера.
    if (via) {
        const name = named(input.current) ?? named(placement?.role === 'station' ? placement.name : undefined);
        return {
            tabs: [
                { key: `station:${via}`, kind: 'station', name, tag: 'gateway', on: true },
                { key: 'computer', kind: 'computer', tag: role.helping ? 'turbo' : undefined, on: false, href: `${path}?local${hash}` },
            ],
            here: 'station',
        };
    }
    // Свой пульт компьютера, что помогает колонке (или ведёт её): колонки — их пульты через проход, компьютер — нажат.
    if (role.role === 'helper') {
        const stations = (placement?.stations ?? (placement?.station ? [placement.station] : [])).filter((entry) => entry.placement !== 'pc');
        const shown = stations.length ? stations : role.station ? [role.station] : [];
        return {
            tabs: [
                ...shown.map((entry) => ({
                    key: `station:${entry.key}`,
                    kind: 'station' as const,
                    name: named(entry.name),
                    tag: 'gateway' as const,
                    on: false,
                    href: `${path}?via=${encodeURIComponent(entry.key)}${hash}`,
                    off: !entry.reachable,
                })),
                { key: 'computer', kind: 'computer', tag: role.helping ? 'turbo' : undefined, on: true },
            ],
            here: 'computer',
        };
    }
    // Колонка со своим шлюзом, открытая браузером: она и её Турбо.
    if (placement?.role === 'station')
        return { tabs: [{ key: 'station', kind: 'station', name: named(input.current) ?? named(placement.name), tag: 'gateway', on: true }, ...turboTabs(placement)], here: 'station' };
    // Шлюз на компьютере: в «Микрофоне» — он сам (со встроенным Турбо — «шлюз + Турбо»), с колонками — выбранная колонка.
    const builtin = (placement?.helpers?.list ?? []).some((entry) => entry.builtin);
    const self: DeviceTab = computer
        ? { key: 'computer', kind: 'computer', tag: builtin ? 'gatewayTurbo' : 'gateway', on: true }
        : { key: 'station', kind: 'station', name: named(input.current), on: true, pick: true };
    return { tabs: [self, ...turboTabs(placement)], here: placement ? 'computer' : undefined };
}

/** Роль на странице: стили рамки читают `<html data-role>`. */
export function applyRole(role: Role): void {
    const root = document.documentElement;
    if (root.dataset.role !== role) root.dataset.role = role;
}
