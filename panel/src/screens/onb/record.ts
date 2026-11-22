// Запись «Первых шагов» (замечание 10 теста 19.11, владелец: «онбординг записывать… чтобы каждый шаг записывался; даже
// если что-то выключил, что-то переделал — восстановить и продолжить»). Одна запись в памяти пульта (shell/memory.ts →
// шлюз этого компьютера, `/api/v1/panel/state`, по человеку; копия в браузере): шаг и выбор, пройденные шаги и хвосты,
// что доделываются в фоне, — приглашение с набором (onb/invite-state.ts) и начатое подключение колонки
// (onb/speaker.tsx). Выбранный персонаж, что ждёт колонку, — в своей записи браузера (magicplace/pending-install.tsx):
// её видит и окно колонки. Перезапустили приложение, шлюз или колонку — шаги открываются на том же шаге со всем
// выбранным, хвосты доводятся сами. Нет записи — шаги пройдены или закрыты.
//
// Пишется слиянием: шаг не стирает хвосты, хвост — шаг. Хвост без открытых шагов записи не заводит: иначе пройденные
// шаги открылись бы снова.

import { recalled, remember } from '../../shell/memory';

export const RECORD = 'onboarding';

/** Метка начатого подключения колонки: какая, куда переносим, когда начали. */
export interface LinkMark {
    address: string;
    name: string;
    to: 'station' | 'stationServer';
    at: number;
}

export interface OnbRecord {
    /** Где живёт (onb/nav.tsx `Choice`). */
    choice: string;
    /** Шаг, на котором остановились. */
    step: string;
    /** false — место выбрал не человек, а пульт (такой выбор не держим). */
    picked?: boolean;
    /** Шаги, пройденные «Дальше», — и при «Пройти заново» то, что в доме уже сделано: в полосе шагов — ✓. */
    done?: string[];
    /** «Пройти заново»: дом уже настроен, шаги — поверх, данные дома не трогаются. */
    again?: boolean;
    /** Приглашение: ссылка, где прочитано, дошло ли до колонки (onb/invite-state.ts). */
    invite?: unknown;
    /** Начатое подключение колонки (onb/speaker.tsx). */
    link?: LinkMark | null;
    /**
     * Компьютер вошёл в уже настроенный дом (центр — колонка): его шаги — «Что даёт этот компьютер», потом «Ваш дом»
     * (`status`). Держится в записи: после входа дом копируется на компьютер вместе с людьми, пульт просит войти снова —
     * и шаги открываются с этого места, а не с «Где живёт» (замечание 5 после 0.18.1).
     */
    joined?: { key: string; name: string; status?: boolean };
}

/** Запись сейчас; нет — шаги не открыты. */
export function readRecord(): OnbRecord | undefined {
    const kept = recalled<OnbRecord>(RECORD);
    return kept && typeof kept === 'object' ? kept : undefined;
}

/** Завести запись (шаги открыли): с тем, что известно; прежняя, если есть, остаётся, — дописывается. */
export function openRecord(start: OnbRecord): void {
    remember(RECORD, { ...start, ...readRecord() } satisfies OnbRecord);
}

/** Дописать в запись — только пока шаги открыты (запись есть). */
export function patchRecord(part: Partial<OnbRecord>): void {
    const now = readRecord();
    if (!now) return;
    remember(RECORD, { ...now, ...part } satisfies OnbRecord);
}

/** Шаги пройдены или закрыты: записи больше нет. */
export function closeRecord(): void {
    remember(RECORD, undefined);
}
