// Типы словарей (docs/plans/i18n.md): перевод повторяет исходный русский словарь — те же ключи, у функций те же
// параметры. Отдельно от index.ts, чтобы словари брали типы без круга импортов.

/** Перевод словаря: те же ключи, у функций — те же параметры. */
export type Translation<T> = {
    [K in keyof T]: T[K] extends string ? string : T[K] extends (...args: infer A) => infer R ? (...args: A) => R : Translation<T[K]>;
};

/** Неполный перевод (язык в работе): чего нет — берётся из английского, затем из русского. */
export type PartialTranslation<T> = {
    [K in keyof T]?: T[K] extends string ? string : T[K] extends (...args: infer A) => infer R ? (...args: A) => R : PartialTranslation<T[K]>;
};
