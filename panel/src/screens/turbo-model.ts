// Турбо без отрисовки (решение владельца 16.11, «Роли DuoWave»): какой Турбо показать на сцене стола и в «Доме», что он
// даёт сейчас и чем шлюз заменит его силы, если он уснёт. Источник — ответ шлюза `/api/v1/helpers` (список и «где
// сейчас») и то, что пульт уже знает: режим колонки, регулятор, облачные слух и голос по ключу. Без DOM и без слов —
// panel/scripts/panel-check.ts проверяет это как есть; слова ставит экран.

import type { Ability, AbilityView, Away, HelperView, HelpersView, PanelMode, WhereRow } from '../api';

/** Силы, которые Турбо даёт вопросу: слышит, думает, говорит. */
export const TURBO_ROLES: Ability[] = ['hearing', 'model', 'voice'];

export interface TurboView {
    helper: HelperView;
    /** Встроенный Турбо этого компьютера (тяжёлая часть приложения), не отдельное устройство. */
    builtin: boolean;
    /** На связи (встроенный — всегда здесь, пока отвечает его сервер). */
    up: boolean;
    /** Что он даёт сейчас — по строкам «где сейчас». */
    gives: Ability[];
    /** Объявленные силы, которые пульт знает, по порядку «модель, слух, голос, …». */
    abilities: AbilityView[];
}

const ORDER: Ability[] = ['model', 'hearing', 'voice', 'apps', 'messages', 'history', 'catalog'];

const givenBy = (now: WhereRow[], id: string): Ability[] => now.filter((row) => row.by === 'helper' && row.helper === id).map((row) => row.ability);

const declared = (helper: HelperView): AbilityView[] =>
    ORDER.flatMap((id) => helper.abilities.filter((ability) => ability.id === id));

/**
 * Турбо дома: из включённых (кроме встроенного) — первый, что даёт что-то сейчас, иначе первый по порядку; таких нет —
 * встроенный, если он даёт слух, модель или голос. Нет и его — Турбо нет.
 */
export function turboOf(view: HelpersView | undefined): TurboView | undefined {
    if (!view) return undefined;
    const others = view.helpers.filter((helper) => !helper.builtin && helper.enabled && helper.state !== 'self' && helper.state !== 'twin');
    const giving = others.find((helper) => givenBy(view.now, helper.id).length > 0);
    const chosen = giving ?? others[0];
    if (chosen)
        return { helper: chosen, builtin: false, up: chosen.state === 'up', gives: givenBy(view.now, chosen.id), abilities: declared(chosen) };
    const own = view.helpers.find((helper) => helper.builtin);
    if (!own) return undefined;
    const gives = givenBy(view.now, own.id);
    if (!gives.some((ability) => TURBO_ROLES.includes(ability))) return undefined;
    return { helper: own, builtin: true, up: true, gives, abilities: declared(own) };
}

/** Кто станет вместо Турбо: облако по ключу, Яндекс (слух) или Алиса (голос), никто; в «Микрофоне» голос — текстом. */
export type FallBy = 'cloud' | 'yandex' | 'none' | 'text';

export interface FallInput {
    /** Шлюз на колонке (там есть Алиса и Яндекс); иначе — компьютер. */
    station: boolean;
    /** Режим «Микрофон»: колонки нет. */
    computer?: boolean;
    mode?: PanelMode;
    away: Away;
    /** Облачный слух и облачный голос по ключу подключены и работают. */
    cloudEar: boolean;
    cloudVoice: boolean;
}

/**
 * Чем шлюз заменит слух или голос Турбо, когда тот уснёт (цепочка шлюза, gateway/src/helpers/now.rs: Турбо → облако
 * по ключу, если его пускает регулятор, → Яндекс, если его пускает режим). «Ваш ИИ» на колонке держится только на
 * облачных слухе и голосе вместе; без них шлюз сам переходит в «Алиса + ваш ИИ» (`modeFalls`) — тогда недостающее
 * слышит Яндекс и говорит Алиса.
 */
export function fallOf(ability: 'hearing' | 'voice', input: FallInput): { by: FallBy; modeFalls: boolean } {
    const ear = input.cloudEar && input.away === 'any';
    const voice = input.cloudVoice && input.away === 'any';
    const modeFalls = input.station && input.mode === 'own' && !(ear && voice);
    if (ability === 'hearing' ? ear : voice) return { by: 'cloud', modeFalls };
    if (!input.station) return { by: ability === 'voice' && input.computer ? 'text' : 'none', modeFalls: false };
    return { by: 'yandex', modeFalls };
}

/** Строка под сценой «Если Турбо уснёт: Думает — …, Слышит — …, Говорит — …»: что встанет вместо каждой его силы. */
export interface FallItem {
    ability: 'model' | 'hearing' | 'voice';
    /** `backup` — «Запасная» из «Дома»; `alice` — на колонке думает Алиса. */
    by: 'backup' | 'alice' | FallBy;
}

/**
 * Строка «Если Турбо уснёт» — только у Турбо, что на связи и даёт вопросу слух, модель или голос (встроенный — сам этот
 * компьютер: он не «засыпает» отдельно от шлюза). `backup` — запасная модель ответит (не на том же Турбо, её пускает
 * регулятор).
 */
export function fallLine(turbo: TurboView | undefined, input: FallInput & { backup: boolean }): { items: FallItem[]; modeFalls: boolean } | null {
    if (!turbo || turbo.builtin || !turbo.up) return null;
    const items: FallItem[] = [];
    let modeFalls = false;
    for (const ability of ['model', 'hearing', 'voice'] as const) {
        if (!turbo.gives.includes(ability)) continue;
        if (ability === 'model') {
            items.push({ ability, by: input.backup ? 'backup' : input.station && input.mode !== 'own' ? 'alice' : 'none' });
            continue;
        }
        const fall = fallOf(ability, input);
        modeFalls ||= fall.modeFalls;
        items.push({ ability, by: fall.by });
    }
    return items.length ? { items, modeFalls } : null;
}
