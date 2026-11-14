// Фильтры MagicPlace (docs/plans/magicplace.md): вид (персонажи, приложения, темы), у приложений — лицо (виджет, навык,
// окно) и где работает (в шлюзе, на компьютере, в сети), состояние (установлено, нет, есть обновление). Людям —
// «приложение»: плагин и приложение со своей программой — одно «Приложения», место говорит, где оно работает.

import type { CatalogEntry, CatalogFace, CatalogPlace } from '../api';
import { tr } from '../i18n';
import { isBlocked, isInstalled } from '../screens/catalog';

export type KindFilter = 'character' | 'app' | 'theme';
export type StateFilter = 'installed' | 'available' | 'update';

export interface Filters {
    kind: KindFilter | null;
    face: CatalogFace | null;
    place: CatalogPlace | null;
    state: StateFilter | null;
}

export const NO_FILTERS: Filters = { kind: null, face: null, place: null, state: null };

/** «Новое» — появилось в каталоге за столько дней. */
export const NEW_DAYS = 30;

const KINDS: KindFilter[] = ['character', 'app', 'theme'];
const FACES: CatalogFace[] = ['widget', 'skill', 'window'];
const PLACES: CatalogPlace[] = ['gateway', 'computer', 'network'];
const STATES: StateFilter[] = ['installed', 'available', 'update'];

/** Вид пакета для людей: плагин и приложение — оба «приложение». */
export function kindOf(entry: CatalogEntry): KindFilter {
    return entry.kind === 'character' ? 'character' : entry.kind === 'theme' ? 'theme' : 'app';
}

/** Состояние пакета для фильтра. */
export function stateOf(entry: CatalogEntry): StateFilter {
    if (entry.state === 'update') return 'update';
    return isInstalled(entry) ? 'installed' : 'available';
}

/** Где работает: от шлюза, а у шлюза постарше — по виду и движку. */
export function placeOfEntry(entry: CatalogEntry): CatalogPlace | null {
    if (entry.place) return entry.place;
    if (entry.kind === 'plugin') return entry.runtime === 'mcp' ? 'network' : 'gateway';
    if (entry.kind === 'app') return 'computer';
    return null;
}

/** Подходит ли пакет к фильтрам. */
export function passes(entry: CatalogEntry, filters: Filters): boolean {
    if (filters.kind && kindOf(entry) !== filters.kind) return false;
    if (filters.face && !(entry.faces ?? []).includes(filters.face)) return false;
    if (filters.place && placeOfEntry(entry) !== filters.place) return false;
    if (filters.state && stateOf(entry) !== filters.state) return false;
    return true;
}

/** Есть ли хоть один фильтр. */
export function filtering(filters: Filters): boolean {
    return !!(filters.kind || filters.face || filters.place || filters.state);
}

/** Появился ли пакет в каталоге за последние `NEW_DAYS` дней. */
export function isNew(entry: CatalogEntry, now = Date.now()): boolean {
    if (!entry.addedAt) return false;
    const at = Date.parse(entry.addedAt);
    return Number.isFinite(at) && now - at <= NEW_DAYS * 24 * 60 * 60 * 1000;
}

/** Встанет ли пакет здесь: не «не пойдёт здесь» и не «нужна подпись» (или он уже стоит). */
export function goesHere(entry: CatalogEntry): boolean {
    return isInstalled(entry) || !isBlocked(entry);
}

/** Фильтры из ссылки: `#/magicplace/store?kind=character&face=widget`. */
export function filtersFrom(query: Record<string, string>): Filters {
    const pick = <T extends string>(value: string | undefined, allowed: T[]): T | null =>
        value && (allowed as string[]).includes(value) ? (value as T) : null;
    return {
        kind: pick(query.kind, KINDS),
        face: pick(query.face, FACES),
        place: pick(query.place, PLACES),
        state: pick(query.state, STATES),
    };
}

/** Слова вида, лица и места пакета на языке пульта — по ним тоже ищут («виджет», «персонаж»). */
export function filterWords(entry: CatalogEntry): string[] {
    const words: string[] = [kindTitle(kindOf(entry))];
    for (const face of entry.faces ?? []) words.push(faceTitle(face));
    const place = placeOfEntry(entry);
    if (place) words.push(placeTitle(place));
    return words;
}

export function kindTitle(kind: KindFilter): string {
    return kind === 'character' ? tr.magicplace.kindCharacters : kind === 'theme' ? tr.magicplace.kindThemes : tr.magicplace.kindApps;
}

export function faceTitle(face: CatalogFace): string {
    switch (face) {
        case 'widget':
            return tr.magicplace.faceWidget;
        case 'skill':
            return tr.magicplace.faceSkill;
        case 'window':
            return tr.magicplace.faceWindow;
        default:
            return tr.magicplace.faceSchedule;
    }
}

export function placeTitle(place: CatalogPlace): string {
    return place === 'gateway' ? tr.magicplace.placeGateway : place === 'network' ? tr.magicplace.placeNetwork : tr.magicplace.placeComputer;
}

export function stateTitle(state: StateFilter): string {
    return state === 'installed' ? tr.magicplace.stateInstalled : state === 'update' ? tr.magicplace.stateUpdate : tr.magicplace.stateNotInstalled;
}

export const kindFilters = (): KindFilter[] => KINDS;
export const faceFilters = (): CatalogFace[] => FACES;
export const placeFilters = (): CatalogPlace[] => PLACES;
export const stateFilters = (): StateFilter[] => STATES;
