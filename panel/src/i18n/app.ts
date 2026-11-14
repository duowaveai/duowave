// Языки для приложений (docs/plans/i18n.md, «SDK окошек»): приложение держит свои тексты словарём по языкам и берёт
// текст на языке пульта. Те же правила, что у `host.i18n` в бэкенде (gateway/src/plugins/quickjs.rs): нет текста на
// языке пульта — английский, затем русский, затем любой.
//
//     const T = { ru: { hello: 'Привет, {name}', items: { one: '{count} дело', few: '{count} дела', many: '{count} дел' } },
//                 en: { hello: 'Hello, {name}', items: { one: '{count} task', other: '{count} tasks' } } };
//     const t = useT(T);
//     t('hello', { name }); t('items', { count: 3 });

import { useMemo } from 'preact/hooks';

import { FALLBACK_LANGUAGE, SOURCE_LANGUAGE } from './languages';
import { PluralForms, fmt } from './format';
import { language } from './state';
import { useLanguage } from '.';

/** Тексты приложения: по коду языка — по ключу строка или формы числа. */
export type AppMessages = Record<string, Record<string, string | PluralForms>>;

const rules = new Map<string, Intl.PluralRules>();

function form(code: string, count: unknown): keyof PluralForms {
    let rule = rules.get(code);
    if (!rule) {
        try {
            rule = new Intl.PluralRules(code);
        } catch {
            rule = new Intl.PluralRules(FALLBACK_LANGUAGE);
        }
        rules.set(code, rule);
    }
    return rule.select(Number(count)) as keyof PluralForms;
}

/** Порядок языков для текста: язык пульта, английский, русский. */
function order(): string[] {
    return [...new Set([language(), FALLBACK_LANGUAGE, SOURCE_LANGUAGE])];
}

/** Текст по ключу из словаря приложения, `{name}` подставлены; нет ни на одном языке — сам ключ. */
export function translate(messages: AppMessages, key: string, params: Record<string, string | number> = {}): string {
    for (const code of [...order(), ...Object.keys(messages)]) {
        const text = messages[code]?.[key];
        if (text === undefined) continue;
        const chosen = typeof text === 'string' ? text : (text[form(code, params.count)] ?? text.other ?? text.many ?? text.one ?? '');
        return fmt(chosen, params);
    }
    return key;
}

/** Текст из вариантов по языкам: `{ru: '…', en: '…'}`. */
export function pickText<T>(variants: Partial<Record<string, T>> | null | undefined): T | undefined {
    if (!variants) return undefined;
    for (const code of order()) if (variants[code] !== undefined) return variants[code];
    return Object.values(variants)[0];
}

/** Язык пульта сейчас (`ru`, `en`…); смена — перерисовка. */
export function useLocale(): string {
    return useLanguage();
}

/** `t(key, params)` по словарю приложения на языке пульта. */
export function useT(messages: AppMessages): (key: string, params?: Record<string, string | number>) => string {
    const code = useLanguage();
    return useMemo(() => (key: string, params?: Record<string, string | number>) => translate(messages, key, params), [messages, code]);
}
