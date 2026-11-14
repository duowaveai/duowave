import type { FunctionComponent } from 'preact';

import type { StationLook } from '../api';
import { tr } from '../i18n';
import { DIVI_HIT, DiviArt, DiviGlyph } from './divi';
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
/** Название семейства — из словаря при каждом обращении (язык меняется на лету). */
const family = (model: Omit<Family, 'title'>, key: string): Family => ({
    Art: model.Art,
    Glyph: model.Glyph,
    get title() {
        return tr.stations.familyTitle[key as keyof typeof tr.stations.familyTitle];
    },
});

const FAMILIES: Record<string, Family> = {
    lite2: family(MODELS.lite2, 'lite2'),
    lite1: family(MODELS.lite1, 'lite1'),
    mini1: family(MODELS.mini1, 'mini1'),
    mini2: family(MODELS.mini2, 'mini2'),
    mini3: family(MODELS.mini3, 'mini3'),
    mini3pro: family(MODELS.mini3pro, 'mini3pro'),
    station: family(MODELS.station, 'station'),
    max: family(MODELS.max, 'max'),
    station2: family(MODELS.station2, 'station2'),
    midi: family(MODELS.midi, 'midi'),
    duomax: family(MODELS.duomax, 'duomax'),
    station3: family(MODELS.station3, 'station3'),
    street: family(MODELS.street, 'street'),
    bork: family(MODELS.bork, 'bork'),
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
    /** Где рисунок нажимается, в координатах его рамки 240×200; без него — корпус Лайт 2. */
    hit?: { x: number; y: number; w: number; h: number; r: number };
}

/**
 * Режим «Микрофон» (docs/plans/mic-mode.md): колонки нет — в центре орбиты ДиВи, знак DuoWave, в той же рамке. Имя —
 * встроенного помощника этого режима.
 */
export const DIVI_DRAWING: StationDrawing = {
    get title() {
        return tr.stations.divi;
    },
    Art: DiviArt,
    Glyph: DiviGlyph,
    color: 'lime',
    clock: false,
    sticker: false,
    drawn: true,
    hit: DIVI_HIT,
};

/** Рисунок по виду, который решил шлюз; без вида — по коду прошивки, иначе — Лайт 2 с часами, синяя. */
export function drawingOf(look: StationLook | null | undefined, deviceModel?: string | null): StationDrawing {
    const raw = deviceModel?.trim() ?? '';
    const familyId = look?.family ?? CODES[raw.toLowerCase()];
    const family = familyId ? FAMILIES[familyId] : undefined;
    return {
        title: look?.title ?? family?.title ?? (raw.replace(/^Yandex\s*/i, '') || tr.stations.speaker),
        Art: family?.Art ?? MODELS.lite2.Art,
        Glyph: family?.Glyph ?? MODELS.lite2.Glyph,
        color: look?.color ?? 'azure',
        clock: look?.clock ?? true,
        sticker: look?.sticker ?? false,
        drawn: !!family,
    };
}
