// Оболочка (Shell) — раскладка пульта: где навигация, верхняя строка и кнопка «Сказать». Какие разделы есть, она не
// знает: пункты навигации приходят готовыми (app.tsx строит их из реестра и роли). Вид выбирает человек
// (theme.ts): стол и док или боковая панель (docs/plans/desk.md).

import type { ComponentChildren, FunctionComponent } from 'preact';

import type { Role } from './role';
import type { Icon, SectionDef } from './sections';

/** Пункт навигации: раздел этого пульта (и его вкладка) или, у помощника, раздел колонки. */
export interface NavEntry {
    /** Уникален в навигации: id раздела, у раздела колонки — `station:<id>`, у вкладки — `<раздел>/<вкладка>`. */
    key: string;
    title: string;
    icon: Icon;
    Badge?: FunctionComponent;
    section: string;
    tab?: string;
    /** Раздел колонки: помощник открывает её пульт окном, правка сохраняется на колонке. */
    station?: boolean;
}

/** Навигация по роли: у помощника слева своё, справа — колонки; «Ещё» — остальное. */
export interface Nav {
    left: NavEntry[];
    right: NavEntry[];
    more: NavEntry[];
}

/** Верхняя строка: роль словом, чья это колонка или компьютер, капсулы режима и связи. */
export interface TopLine {
    role: Role;
    title: string;
    subtitle: ComponentChildren;
    chips: ComponentChildren;
    /** Телефон: подпись и капсулы спрятаны — выбор колонки коротко, её имя с точкой связи. */
    narrow?: ComponentChildren;
}

/** Открытый раздел, кроме стола: окно поверх стола (или страница в боковой панели). */
export interface OpenWindow {
    /**
     * Какое это окно: чьё (у помощника — своё или колонки) и какой раздел. Пока он тот же, окно то же — и при смене
     * вкладки, и когда пункт дока меняется («Плагины компьютера» ↔ «Мастерская»).
     */
    id: string;
    /** Пункт навигации, который открыл окно. */
    key: string;
    title: string;
    /** Чьё окно: колонки (полоса «сохраняется на колонке») или этого компьютера (тёмная метка) — у помощника. */
    scope?: 'station' | 'own';
    /** Метка в заголовке окна: у окна колонки — её имя и связь. */
    badge?: ComponentChildren;
    content: ComponentChildren;
    onClose: () => void;
}

export interface ShellProps {
    /** Разделы, которые видит этот вход, по порядку. */
    sections: SectionDef[];
    nav: Nav;
    /** Открытый пункт навигации (key). */
    active: string;
    onNav: (entry: NavEntry) => void;
    top: TopLine;
    /** Переключатель колонки (имя, состояние, выбор). */
    station: ComponentChildren;
    /** Кнопки шапки: вклады слота header.actions и смена светлой и тёмной. */
    actions: ComponentChildren;
    /** «Сказать через колонку» — одна лаймовая кнопка на экран (DESIGN.md). */
    talk: { disabled: boolean; onTalk: () => void };
    /** «+» — новый виджет на стол. */
    onAdd: () => void;
    /** Плашка над содержимым: шлюз недоступен. */
    notice: ComponentChildren;
    /** Стол: колонка и виджеты. */
    desk: ComponentChildren;
    /** Открытый раздел поверх стола; null — стол. */
    window: OpenWindow | null;
}

/** Разделы для основной навигации и для «ещё» (журнал, настройки: в доке им нет места). */
export function splitNav(sections: SectionDef[]): { main: SectionDef[]; more: SectionDef[] } {
    return {
        main: sections.filter((section) => (section.nav ?? 'main') === 'main'),
        more: sections.filter((section) => section.nav === 'more'),
    };
}
