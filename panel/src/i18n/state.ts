// Текущий язык пульта — отдельно от словарей: форматы и плюралы читают его, а словари импортируют форматы, и круга
// импортов нет (index.ts → locales → format.ts → state.ts).

import { SOURCE_LANGUAGE } from './languages';

let current = SOURCE_LANGUAGE;

/** Текущий язык пульта: код BCP 47 (`ru`, `en`). */
export function language(): string {
    return current;
}

export function setCurrentLanguage(code: string): void {
    current = code;
}
