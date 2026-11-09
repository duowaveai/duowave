// Реестр пульта: что принесли плагины — разделы, вклады в слоты, темы. Встроенное (builtin.tsx) приходит сюда тем же
// путём, что потом придут плагины с шлюза (этап 5: import('/plugins/<id>/panel.js') → registerPlugin(модуль.default)).
// Ядро пульта не знает, какие разделы и карточки есть: оболочки и экраны читают их отсюда.

import { useEffect, useState } from 'preact/hooks';

import type { SectionDef } from './sections';
import type { SlotContribution } from './slot';
import type { Theme } from './themes';

/** Что плагин приносит в пульт. Тот же вид у встроенного: builtin.tsx. */
export interface PanelPlugin {
    id: string;
    sections?: SectionDef[];
    /** Вклады по именам слотов: 'home.cards', 'header.actions', 'house.tabs', 'say.extras'. */
    slots?: Record<string, SlotContribution[]>;
    themes?: Theme[];
}

/**
 * Описание плагина пульта. Модуль плагина отдаёт его по умолчанию (`export default definePlugin({…})`), а
 * регистрирует пульт — registerPlugin: так он сам решает, что включено (безопасный режим, выключенные плагины).
 */
export function definePlugin(plugin: PanelPlugin): PanelPlugin {
    if (!plugin.id || !/^[a-z0-9][a-z0-9._-]*$/.test(plugin.id)) throw new Error(`Плагин пульта: id «${plugin.id}» — латиница, цифры, «.», «_», «-»`);
    return plugin;
}

const plugins = new Map<string, PanelPlugin>();
const listeners = new Set<() => void>();
let revision = 0;

const changed = (): void => {
    revision += 1;
    for (const listener of [...listeners]) listener();
};

/** Включить плагин в пульте; тот же id ещё раз — новая версия на месте старой. */
export function registerPlugin(plugin: PanelPlugin): void {
    plugins.set(plugin.id, plugin);
    changed();
}

export function unregisterPlugin(id: string): void {
    if (plugins.delete(id)) changed();
}

export function registeredPlugins(): PanelPlugin[] {
    return [...plugins.values()];
}

/** Подписка на перемены реестра; возвращает отписку. */
export function onRegistryChange(listener: () => void): () => void {
    listeners.add(listener);
    return () => listeners.delete(listener);
}

/** Перерисовать компонент, когда реестр меняется (плагин включили, выключили, обновили). */
export function useRegistry(): number {
    const [seen, setSeen] = useState(revision);
    useEffect(() => {
        const sync = () => setSeen(revision);
        sync();
        return onRegistryChange(sync);
    }, []);
    return seen;
}
