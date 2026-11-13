// Части пульта у приложений на ходу (docs/plans/plugins.md, этап 5; docs/plans/bus.md, «Один способ»): темы пакетов и
// части включённых приложений приходят с шлюза, а не из сборки пульта. Тема — данные, она встаёт в «Вид пульта» сразу.
// Карточки приложения (`contributes.panel.cards`) — данные, их рисует пульт (cards/). Свой `panel.js` у приложения
// работает только в его окошке — изолированном фрейме (frame/host.tsx): пульт узнаёт, что он приносит, и рисует его
// разделы и виджеты окошками. Всё регистрируется тем же registerPlugin, что встроенное в пульт (builtin.tsx), под id
// `package.<id>`: разделы и вклады живут в своих границах ошибок и в его области (PluginScope): `usePanelState().version`
// у них — номер перемены его пакета (plugins/versions.ts). Выключили приложение — его вклады уходят из пульта.
// `?safe=1` в адресе пульта — безопасный режим: ни тем пакетов, ни частей приложений.

import { FunctionComponent, h } from 'preact';

import { CardSpec, PanelPart, api } from '../api';
import { AppCard } from '../cards/card';
import { framedPlugin, probeShape, probeState } from '../frame/host';
import { icons } from '../sdk';
import type { SlotContribution } from '../shell/slot';
import { PanelPlugin, registerPlugin, unregisterPlugin } from '../shell/registry';
import { normalizeTheme } from '../shell/themes';
import { gw } from '../via';
import { PluginScope } from './versions';

/** Безопасный режим пульта: ?safe=1. */
export function panelSafeMode(): boolean {
    try {
        return new URLSearchParams(location.search).get('safe') === '1';
    } catch {
        return false;
    }
}

/** Значения настроек плагинов пульта — для useSettings. */
export const packageSettings = new Map<string, Record<string, unknown>>();

/** Что загружено у каждого пакета: модуль, стили, окошко ли, карточки — одной строкой, чтобы сравнить. */
const loaded = new Map<string, string>();
const THEMES_ID = 'packages.themes';
const pluginKey = (id: string): string => `package.${id}`;

/** Включён ли плагин на этом шлюзе: его часть пульта загружена (у помощника — компьютера). */
export function enabledHere(id: string): boolean {
    return loaded.has(id);
}

/** Компонент плагина в его области: SDK знает, чей он (usePanelState). */
function inScope<P>(id: string, Component: FunctionComponent<P>): FunctionComponent<P> {
    const Scoped: FunctionComponent<P> = (props) => h(PluginScope.Provider, { value: id }, h(Component as FunctionComponent<object>, props as object));
    Scoped.displayName = `${id}:${Component.displayName || Component.name || 'вклад'}`;
    return Scoped;
}

/** Разделы и вклады плагина — каждый в его области. */
function scoped(id: string, plugin: PanelPlugin): PanelPlugin {
    return {
        ...plugin,
        sections: plugin.sections?.map((section) => ({
            ...section,
            Screen: inScope(id, section.Screen),
            Badge: section.Badge && inScope(id, section.Badge),
        })),
        slots:
            plugin.slots &&
            Object.fromEntries(
                Object.entries(plugin.slots).map(([name, list]) => [
                    name,
                    Array.isArray(list) ? list.map((entry) => (entry?.Component ? { ...entry, Component: inScope(id, entry.Component) } : entry)) : list,
                ]),
            ),
    };
}

async function syncThemes(): Promise<void> {
    try {
        const { themes } = await api.packageThemes();
        const list = themes.map((entry) => normalizeTheme(entry.theme)).filter((theme) => theme !== null);
        if (list.length > 0) registerPlugin({ id: THEMES_ID, themes: list });
        else unregisterPlugin(THEMES_ID);
    } catch {
        // шлюз постарше или нет права — темы пакетов просто не показываем
    }
}

/** Карточки приложения — виджеты стола: данные от его функции, рисует пульт. */
function cardWidgets(part: PanelPart): SlotContribution[] {
    return (part.cards ?? [])
        .filter((spec: CardSpec) => spec && typeof spec.id === 'string' && typeof spec.function === 'string')
        .map((spec: CardSpec) => ({
            id: `card.${spec.id}`,
            title: spec.title ?? part.title,
            icon: (spec.icon && (icons as Record<string, SlotContribution['icon']>)[spec.icon]) || undefined,
            size: spec.size === 'large' ? 'large' : 'small',
            initial: spec.initial ?? false,
            // Чья карточка — подписью справа; совпадает с её заголовком — просто «приложение», как у прежних виджетов.
            source: (spec.title ?? part.title) === part.title ? 'приложение' : part.title,
            Component: (() => h(AppCard, { app: part.id, spec })) as SlotContribution['Component'],
        }));
}

/** Модуль пульта приложения — всегда в его окошке: пульт узнаёт разведкой, что он приносит, и рисует окошками. */
async function moduleOf(part: PanelPart): Promise<PanelPlugin> {
    if (!part.module) return { id: pluginKey(part.id) };
    const module = gw(part.module);
    const styles = part.styles ? gw(part.styles) : null;
    const shape = await probeShape(part.id, module, styles, probeState(part.id, part.settings ?? {}));
    return framedPlugin(pluginKey(part.id), part.id, module, styles, shape);
}

async function syncParts(): Promise<void> {
    let parts;
    try {
        parts = (await api.packagePanelParts()).parts;
    } catch {
        return;
    }
    const seen = new Set<string>();
    for (const part of parts) {
        seen.add(part.id);
        packageSettings.set(part.id, part.settings ?? {});
        const signature = JSON.stringify([part.module ?? null, part.styles ?? null, part.cards ?? []]);
        if (loaded.get(part.id) === signature) continue;
        loaded.set(part.id, signature);
        let plugin: PanelPlugin = { id: pluginKey(part.id) };
        try {
            plugin = await moduleOf(part);
        } catch (error) {
            console.error(`Пульт: плагин «${part.title}» (${part.id} ${part.version}) не загрузился`, error);
        }
        const cards = cardWidgets(part);
        if (cards.length > 0) plugin = { ...plugin, slots: { ...plugin.slots, 'desk.widgets': [...(plugin.slots?.['desk.widgets'] ?? []), ...cards] } };
        const brings = (plugin.sections?.length ?? 0) > 0 || Object.values(plugin.slots ?? {}).some((list) => list.length > 0) || (plugin.themes?.length ?? 0) > 0;
        if (brings) registerPlugin(scoped(part.id, plugin));
        else unregisterPlugin(pluginKey(part.id));
    }
    for (const id of [...loaded.keys()]) {
        if (seen.has(id)) continue;
        loaded.delete(id);
        packageSettings.delete(id);
        unregisterPlugin(pluginKey(id));
    }
}

/** Привести пульт в согласие с пакетами шлюза: темы и модули плагинов. */
export async function syncPackages(): Promise<void> {
    if (panelSafeMode()) return;
    await Promise.all([syncThemes(), syncParts()]);
}
