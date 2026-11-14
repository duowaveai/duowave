// Плюралы, числа, даты и списки по правилам текущего языка пульта (docs/plans/i18n.md). Словари зовут их из своих
// функций: `count: (n: number) => \`${n} ${plural(n, {one: 'колонка', few: 'колонки', many: 'колонок'})}\``.

import { FALLBACK_LANGUAGE, SOURCE_LANGUAGE } from './languages';
import { language } from './state';

/** Формы числа: `one` 1, `few` 2–4 (русский), `many` 5–20, `other` — дробные и всё у языков без форм. */
export interface PluralForms {
    zero?: string;
    one?: string;
    two?: string;
    few?: string;
    many?: string;
    other?: string;
}

const pluralRules = new Map<string, Intl.PluralRules>();

/**
 * Слово при числе по правилам текущего языка: `plural(3, {one: 'колонка', few: 'колонки', many: 'колонок'})`. Числа в
 * форме нет — его пишет словарь. Формы, которой язык не дал, нет — берётся `other`, затем `many`, затем `one`.
 */
export function plural(count: number, forms: PluralForms): string {
    const code = language();
    let rules = pluralRules.get(code);
    if (!rules) {
        rules = new Intl.PluralRules(code);
        pluralRules.set(code, rules);
    }
    const category = rules.select(count) as keyof PluralForms;
    return forms[category] ?? forms.other ?? forms.many ?? forms.one ?? '';
}

/** Подстановка `{name}` в строку: для текстов, что приходят строкой (приложения, шлюз). */
export function fmt(text: string, params: Record<string, string | number> = {}): string {
    return text.replace(/\{(\w+)\}/g, (whole, key: string) => (key in params ? String(params[key]) : whole));
}

/** Локаль для `Intl` и `toLocale…String`: язык пульта. */
export function locale(): string {
    return language();
}

/** Число по правилам языка пульта: разряды, дробная часть. */
export function formatNumber(value: number, options?: Intl.NumberFormatOptions): string {
    return new Intl.NumberFormat(language(), options).format(value);
}

/** Дата и время по правилам языка пульта. */
export function formatDate(value: Date | number | string, options?: Intl.DateTimeFormatOptions): string {
    return new Intl.DateTimeFormat(language(), options).format(typeof value === 'string' ? new Date(value) : value);
}

/** Список «a, b и c» по правилам языка. */
export function formatList(items: string[], type: 'conjunction' | 'disjunction' = 'conjunction'): string {
    return new Intl.ListFormat(language(), { type }).format(items);
}

/**
 * Текст из вариантов по языкам — для того, что пришло со шлюза или из приложения: `{ru: '…', en: '…'}`. Нет
 * текущего — английский, затем русский, затем первый попавшийся.
 */
export function pick<T>(variants: Partial<Record<string, T>> | null | undefined): T | undefined {
    if (!variants) return undefined;
    const code = language();
    return variants[code] ?? variants[FALLBACK_LANGUAGE] ?? variants[SOURCE_LANGUAGE] ?? Object.values(variants)[0];
}
