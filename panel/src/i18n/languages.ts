// Языки пульта (docs/plans/i18n.md). Новый язык — папка `locales/<код>/` (копия `en/`, переведённая) и одна строка
// здесь: он появится в выборе языка, его словарь загрузится, когда его выберут, а запросы к шлюзу понесут его код.

export interface LanguageInfo {
    /** Основной подтег BCP 47: `ru`, `en`, `de`… */
    code: string;
    /** Самоназвание: так язык подписан в выборе на любом языке пульта. */
    name: string;
    /** Название по-английски: для поиска и подсказок. */
    english: string;
    /** Письмо справа налево. */
    rtl?: boolean;
}

export const LANGUAGES: LanguageInfo[] = [
    { code: 'ru', name: 'Русский', english: 'Russian' },
    { code: 'en', name: 'English', english: 'English' },
];

/** Язык исходных текстов: его словарь полный и всегда в сборке. */
export const SOURCE_LANGUAGE = 'ru';

/** Язык, когда система человека говорит на языке, которого у пульта нет. */
export const FALLBACK_LANGUAGE = 'en';

/** `en-GB` → `en`; не код языка — null. */
export function baseOf(tag: string | null | undefined): string | null {
    const base = (tag ?? '').trim().toLowerCase().split(/[-_]/)[0];
    return /^[a-z]{2,3}$/.test(base) ? base : null;
}

export function isSupported(code: string | null | undefined): code is string {
    return !!code && LANGUAGES.some((language) => language.code === code);
}

export function languageInfo(code: string): LanguageInfo | undefined {
    return LANGUAGES.find((language) => language.code === code);
}
