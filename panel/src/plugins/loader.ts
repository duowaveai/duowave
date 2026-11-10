// Плагины пульта на ходу (docs/plans/plugins.md, этап 5): темы пакетов и `panel.js` включённых плагинов приходят с
// шлюза, а не из сборки пульта. Тема — данные, она встаёт в «Вид пульта» сразу; модуль плагина подключается через
// import() и регистрируется тем же registerPlugin, что встроенное (builtin.tsx), под id `package.<id>`: его разделы и
// вклады в слоты живут в своих границах ошибок и в его области (PluginScope): `usePanelState().version` у них — номер
// перемены его пакета (plugins/versions.ts). Выключили плагин — его вклады уходят из пульта. `?safe=1` в адресе
// пульта — безопасный режим: ни тем пакетов, ни модулей плагинов.

import { FunctionComponent, h } from 'preact';

import { api } from '../api';
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

const loaded = new Map<string, { module?: string | null; styles?: string | null }>();
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

function setStyles(id: string, href?: string | null): void {
    const selector = `link[data-package="${CSS.escape(id)}"]`;
    const current = document.head.querySelector<HTMLLinkElement>(selector);
    if (!href) {
        current?.remove();
        return;
    }
    if (current?.getAttribute('href') === href) return;
    const link = current ?? document.createElement('link');
    link.rel = 'stylesheet';
    link.dataset.package = id;
    link.href = href;
    if (!current) document.head.appendChild(link);
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
        const before = loaded.get(part.id);
        if (before && before.module === part.module && before.styles === part.styles) continue;
        loaded.set(part.id, { module: part.module, styles: part.styles });
        setStyles(part.id, part.styles ? gw(part.styles) : part.styles);
        if (!part.module) {
            unregisterPlugin(pluginKey(part.id));
            continue;
        }
        try {
            const module = await import(/* @vite-ignore */ gw(part.module));
            const plugin = module.default ?? module.plugin;
            if (!plugin || typeof plugin !== 'object') throw new Error('модуль не отдаёт плагин: export default definePlugin({…})');
            registerPlugin(scoped(part.id, { ...plugin, id: pluginKey(part.id) }));
        } catch (error) {
            console.error(`Пульт: плагин «${part.title}» (${part.id} ${part.version}) не загрузился`, error);
            unregisterPlugin(pluginKey(part.id));
        }
    }
    for (const id of [...loaded.keys()]) {
        if (seen.has(id)) continue;
        loaded.delete(id);
        packageSettings.delete(id);
        setStyles(id, null);
        unregisterPlugin(pluginKey(id));
    }
}

/** Привести пульт в согласие с пакетами шлюза: темы и модули плагинов. */
export async function syncPackages(): Promise<void> {
    if (panelSafeMode()) return;
    await Promise.all([syncThemes(), syncParts()]);
}
