import { useEffect, useState } from 'preact/hooks';

import type { FeatureId } from './availability-ids';

export type { FeatureId } from './availability-ids';

/**
 * Что где доступно — список шлюза (gateway/src/core/availability.rs), один на оба пульта: доступна ли функция здесь
 * и сейчас, где она работает (здесь, на компьютере, в облаке, у Яндекса) и почему нет. Пульт своих списков не держит:
 * приглушить кнопку — `available('…')`, объяснить — `whyNot('…')`.
 */

export type Placement = 'pc' | 'station' | 'stationServer';
export type Runs = 'here' | 'homeServer' | 'cloud' | 'yandex';
export type FeatureLevel = 'yes' | 'partial' | 'no';
export type HomeServerState = 'none' | 'up' | 'down';

export interface FeatureState {
    id: FeatureId;
    title: string;
    group: string;
    /** ready — работает первым путём; fallback — запасным («компьютер не на связи — через облако»); off — никак. */
    state: 'ready' | 'fallback' | 'off';
    runs?: Runs;
    reason: string;
}

export interface FeatureCell {
    level: FeatureLevel;
    note: string;
}

export interface FeatureRow {
    id: FeatureId;
    title: string;
    group: string;
    pc: FeatureCell;
    station: FeatureCell;
    stationServer: FeatureCell;
}

export interface PlacementCard {
    id: Placement;
    title: string;
    summary: string;
    risk: string;
}

export interface AvailabilityView {
    placement: Placement;
    homeServer: HomeServerState;
    features: FeatureState[];
    placements: PlacementCard[];
    table: FeatureRow[];
}

export const GROUP_TITLES: Record<string, string> = {
    modes: 'Режимы',
    voice: 'Голос',
    brain: 'Мозг',
    skills: 'Навыки',
    house: 'Дом',
    system: 'Система',
};

export const RUNS_TITLES: Record<Runs, string> = {
    here: 'здесь',
    homeServer: 'на компьютере',
    cloud: 'в облаке',
    yandex: 'у Яндекса',
};

export const PLACEMENT_TITLES: Record<Placement, string> = {
    pc: 'На компьютере',
    station: 'На колонке',
    stationServer: 'Колонка + ПК',
};

let current: AvailabilityView | null = null;
const listeners = new Set<(view: AvailabilityView | null) => void>();

/** Пульт запоминает ответ шлюза; шлюз постарше списка не знает — тогда доступно всё, как раньше. */
export function noteAvailability(view: AvailabilityView | null): void {
    current = view;
    for (const listener of listeners) listener(view);
}

export function availability(): AvailabilityView | null {
    return current;
}

export function featureState(id: FeatureId): FeatureState | undefined {
    return current?.features.find((feature) => feature.id === id);
}

/** Доступна ли функция сейчас. Пока шлюз не ответил (или он постарше) — да: приглушать нечего. */
export function available(id: FeatureId): boolean {
    const feature = featureState(id);
    return feature ? feature.state !== 'off' : true;
}

/** Работает ли функция в этом самом шлюзе (а не на компьютере рядом или в облаке): кнопки, которые делают её здесь. */
export function here(id: FeatureId): boolean {
    const feature = featureState(id);
    return feature ? feature.state !== 'off' && feature.runs === 'here' : true;
}

/** Почему функция недоступна — словами шлюза; доступна — ничего. */
export function whyNot(id: FeatureId): string | undefined {
    const feature = featureState(id);
    return feature && feature.state === 'off' ? feature.reason : undefined;
}

/** Работает ли запасным путём, и каким: «компьютер не на связи — через облако». */
export function fallbackNote(id: FeatureId): string | undefined {
    const feature = featureState(id);
    return feature?.state === 'fallback' ? feature.reason : undefined;
}

/** Ответ шлюза, перечитанный при каждой его перемене. */
export function useAvailability(): AvailabilityView | null {
    const [view, setView] = useState(current);
    useEffect(() => {
        listeners.add(setView);
        setView(current);
        return () => {
            listeners.delete(setView);
        };
    }, []);
    return view;
}
