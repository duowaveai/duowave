/// <reference types="vite/client" />
// Языки пульта (docs/plans/i18n.md): словарь текущего языка — `tr`, его смена без перезагрузки, выбор языка при первом
// запуске; плюралы, даты и числа по языку — format.ts.
//
// Как писать тексты:
//
//     import { tr } from '../i18n';
//     <h2>{tr.settings.languageTitle}</h2>
//     <p>{tr.stations.count(stations.length)}</p>
//
// `tr` — живая привязка модуля: после `setLanguage` она указывает на словарь нового языка, а корень пульта
// перерисовывается заново. Поэтому текст читают при отрисовке или в функции — **не в константе модуля**: константа
// запомнит язык, с которым модуль загрузился.
//
// Словари — `locales/<код>/`: русский — исходный и полный, остальные проверяются по его типу (`Translation`).

import { useEffect, useState } from 'preact/hooks';

import ru, { Messages } from './locales/ru';
import { FALLBACK_LANGUAGE, SOURCE_LANGUAGE, baseOf, isSupported, languageInfo } from './languages';
import { language, setCurrentLanguage } from './state';

export { LANGUAGES, languageInfo, baseOf, isSupported, SOURCE_LANGUAGE, FALLBACK_LANGUAGE } from './languages';
export type { LanguageInfo } from './languages';
export type { Messages } from './locales/ru';
export { language } from './state';
export { plural, fmt, locale, formatNumber, formatDate, formatList, pick } from './format';
export type { PluralForms } from './format';

export type { Translation, PartialTranslation } from './types';
import type { PartialTranslation } from './types';

/** Выбор человека в этом браузере. */
const CHOICE_KEY = 'duowave-language';

/** Словари языков: грузятся, когда язык выбран; исходный — уже в сборке. */
const loaders = import.meta.glob<{ default: PartialTranslation<Messages> }>(['./locales/*/index.ts', '!./locales/ru/index.ts']);

const loaded = new Map<string, Messages>([[SOURCE_LANGUAGE, ru]]);

/** Словарь текущего языка. */
export let tr: Messages = ru;

const listeners = new Set<(code: string) => void>();

function isObject(value: unknown): value is Record<string, unknown> {
    return !!value && typeof value === 'object' && !Array.isArray(value);
}

/** Словарь с запасными: чего нет в `over`, берётся из `base`. */
export function merged<T>(base: T, over: unknown): T {
    if (!isObject(over) || !isObject(base)) return (over ?? base) as T;
    const out: Record<string, unknown> = { ...base };
    for (const [key, value] of Object.entries(over)) {
        out[key] = isObject(value) && isObject(out[key]) ? merged(out[key], value) : (value ?? out[key]);
    }
    return out as T;
}

async function dictionary(code: string): Promise<Messages> {
    const ready = loaded.get(code);
    if (ready) return ready;
    const load = loaders[`./locales/${code}/index.ts`];
    if (!load) return ru;
    const own = (await load()).default;
    // Неполный язык: пропуски — из английского, затем из русского.
    const base = code !== FALLBACK_LANGUAGE && loaders[`./locales/${FALLBACK_LANGUAGE}/index.ts`] ? await dictionary(FALLBACK_LANGUAGE) : ru;
    const full = merged(base, own);
    loaded.set(code, full);
    return full;
}

function readChoice(): string | null {
    try {
        const stored = localStorage.getItem(CHOICE_KEY);
        return isSupported(stored) ? stored : null;
    } catch {
        return null;
    }
}

function writeChoice(code: string | null): void {
    try {
        if (code) localStorage.setItem(CHOICE_KEY, code);
        else localStorage.removeItem(CHOICE_KEY);
    } catch {
        // приватное окно: выбор живёт до перезагрузки
    }
}

/** Выбрал ли человек язык на этом устройстве сам. */
export function hasChoice(): boolean {
    return readChoice() !== null;
}

/** Язык системы человека, который есть у пульта; нет такого — английский. */
export function detectLanguage(
    preferred: readonly string[] = typeof navigator !== 'undefined' ? (navigator.languages ?? [navigator.language]) : [],
): string {
    for (const tag of preferred) {
        const base = baseOf(tag);
        if (isSupported(base)) return base;
    }
    return isSupported(FALLBACK_LANGUAGE) ? FALLBACK_LANGUAGE : SOURCE_LANGUAGE;
}

function apply(code: string, messages: Messages): void {
    setCurrentLanguage(code);
    tr = messages;
    if (typeof document !== 'undefined') {
        document.documentElement.lang = code;
        document.documentElement.dir = languageInfo(code)?.rtl ? 'rtl' : 'ltr';
    }
    for (const listener of listeners) listener(code);
}

/**
 * Сменить язык пульта. `remember` — это выбор человека на этом устройстве (по умолчанию да); без него язык меняется
 * до перезагрузки — так пульт берёт язык дома.
 */
export async function setLanguage(code: string, remember = true): Promise<void> {
    const next = isSupported(code) ? code : SOURCE_LANGUAGE;
    if (remember) writeChoice(next);
    if (next === language()) return;
    apply(next, await dictionary(next));
}

/** Язык дома со шлюза (`GET /api/v1/language`, без входа); шлюз постарше или молчит — null. */
async function householdLanguage(): Promise<string | null> {
    try {
        const controller = new AbortController();
        const timer = setTimeout(() => controller.abort(), 1500);
        const response = await fetch('/api/v1/language', { headers: { Accept: 'application/json' }, signal: controller.signal });
        clearTimeout(timer);
        if (!response.ok) return null;
        const body = (await response.json()) as { message?: { language?: string | null } } | null;
        const code = body?.message?.language;
        return isSupported(code) ? code : null;
    } catch {
        return null;
    }
}

/**
 * Язык при запуске: выбор на этом устройстве, иначе язык дома со шлюза, иначе язык системы. Словарь загружается до
 * первой отрисовки.
 */
export async function initLanguage(): Promise<string> {
    const code = readChoice() ?? (await householdLanguage()) ?? detectLanguage();
    if (code !== language()) apply(code, await dictionary(code));
    else if (typeof document !== 'undefined') document.documentElement.lang = code;
    return code;
}

/** Подписка на смену языка; возвращает отписку. */
export function onLanguageChange(listener: (code: string) => void): () => void {
    listeners.add(listener);
    return () => listeners.delete(listener);
}

/** Текущий язык как состояние компонента: перерисовка при смене. */
export function useLanguage(): string {
    const [code, setCode] = useState(language());
    useEffect(() => onLanguageChange(setCode), []);
    return code;
}
