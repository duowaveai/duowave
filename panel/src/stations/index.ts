import type { FunctionComponent } from 'preact';

import type { StationLook } from '../api';
import { MODELS } from './models';
import type { StationArtProps, StationGlyphProps } from './types';

export type { StationArtProps, StationStage } from './types';
export { COLORS, colorInfo } from './colors';

/** Семейство колонок для пульта: его рисунок и значок. Варианты и цвета решает шлюз (gateway/src/stations/looks.rs). */
interface Family {
    title: string;
    Art: FunctionComponent<StationArtProps>;
    Glyph: FunctionComponent<StationGlyphProps>;
}

/**
 * Семейства по id шлюза — вся линейка Станций; рисунки — models.tsx (корпус на модель, свет и дисплей общие, kit.tsx).
 * Новая модель — её корпус там и её варианты в looks.rs шлюза; дашборд при этом не меняется.
 */
const FAMILIES: Record<string, Family> = {
    lite2: { ...MODELS.lite2, title: 'Лайт 2' },
    lite1: { ...MODELS.lite1, title: 'Лайт' },
    mini1: { ...MODELS.mini1, title: 'Мини' },
    mini2: { ...MODELS.mini2, title: 'Мини 2' },
    mini3: { ...MODELS.mini3, title: 'Мини 3' },
    mini3pro: { ...MODELS.mini3pro, title: 'Мини 3 Про' },
    station: { ...MODELS.station, title: 'Станция' },
    max: { ...MODELS.max, title: 'Макс' },
    station2: { ...MODELS.station2, title: 'Станция 2' },
    midi: { ...MODELS.midi, title: 'Миди' },
    duomax: { ...MODELS.duomax, title: 'Дуо Макс' },
    station3: { ...MODELS.station3, title: 'Станция 3' },
    street: { ...MODELS.street, title: 'Стрит' },
    bork: { ...MODELS.bork, title: 'Станция BORK' },
};

/** Коды прошивки → семейство: для сессии, о которой шлюз постарше ещё не ведёт записи колонки. */
const CODES: Record<string, string> = {
    plum: 'lite2',
    yandexmicro: 'lite1',
    yandexmini: 'mini1',
    yandexmini_2: 'mini2',
    bergamot: 'mini3',
    pickle: 'mini3pro',
    yandexstation: 'station',
    yandexstation_2: 'max',
    yandexmidi: 'station2',
    cucumber: 'midi',
    chiron: 'duomax',
    orion: 'station3',
    mango: 'street',
    bork: 'bork',
};

/** Чем и как нарисовать колонку: рисунок, значок, цвет корпуса и есть ли часы. */
export interface StationDrawing {
    title: string;
    Art: FunctionComponent<StationArtProps>;
    Glyph: FunctionComponent<StationGlyphProps>;
    color: string;
    clock: boolean;
    /** Наклейка-котик на фасаде (выбор человека, только у Лайт 2). */
    sticker: boolean;
    /** Есть ли у модели свой рисунок; у незнакомой — рисунок Лайт 2, пока для неё не нарисован свой. */
    drawn: boolean;
}

/** Рисунок по виду, который решил шлюз; без вида — по коду прошивки, иначе — Лайт 2 с часами, синяя. */
export function drawingOf(look: StationLook | null | undefined, deviceModel?: string | null): StationDrawing {
    const raw = deviceModel?.trim() ?? '';
    const familyId = look?.family ?? CODES[raw.toLowerCase()];
    const family = familyId ? FAMILIES[familyId] : undefined;
    return {
        title: look?.title ?? family?.title ?? (raw.replace(/^Yandex\s*/i, '') || 'Колонка'),
        Art: family?.Art ?? MODELS.lite2.Art,
        Glyph: family?.Glyph ?? MODELS.lite2.Glyph,
        color: look?.color ?? 'azure',
        clock: look?.clock ?? true,
        sticker: look?.sticker ?? false,
        drawn: !!family,
    };
}
