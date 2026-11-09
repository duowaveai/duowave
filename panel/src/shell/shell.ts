// Оболочка (Shell) — раскладка пульта: где навигация, шапка и кнопка «Сказать». Какие разделы есть, она не знает:
// получает их из реестра готовым списком. Тема выбирает оболочку полем layout.

import type { ComponentChildren } from 'preact';

import type { SectionDef } from './sections';

export interface ShellProps {
    /** Разделы, которые видит этот вход, по порядку. */
    sections: SectionDef[];
    /** Открытый раздел. */
    active: string;
    onGo: (section: string) => void;
    /** Переключатель колонки (имя, состояние, выбор) — шапка или боковая панель показывает его первым. */
    station: ComponentChildren;
    /** Кнопки шапки: вклады слота header.actions и смена светлой и тёмной. */
    actions: ComponentChildren;
    /** «Сказать через колонку» — одна лаймовая кнопка на экран (DESIGN.md). */
    talk: { disabled: boolean; onTalk: () => void };
    /** Плашка над содержимым: шлюз недоступен. */
    notice: ComponentChildren;
    /** Экран открытого раздела. */
    children: ComponentChildren;
}

/** Разделы для основной навигации и для «ещё» (боковая панель показывает их ниже, панель вкладок — нет). */
export function splitNav(sections: SectionDef[]): { main: SectionDef[]; more: SectionDef[] } {
    return {
        main: sections.filter((section) => (section.nav ?? 'main') === 'main'),
        more: sections.filter((section) => section.nav === 'more'),
    };
}
