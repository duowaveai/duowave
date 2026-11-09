// SDK пульта для плагинов (docs/plans/plugins.md, этап 5; типы для авторов — docs/plugins/panel-sdk.d.ts). Модуль
// плагина (`panel.js` пакета) импортирует «preact», «preact/hooks» и «@duowave/panel-sdk» — import map в index.html ведёт
// их к файлам public/sdk/, а те берут всё отсюда: у плагина нет своей копии Preact, и его компоненты живут в дереве
// пульта, в своих границах ошибок. Плагин ходит в шлюз только через useApi — к функциям своего бэкенда, которые его
// манифест перечислил в contributes.panel.calls; права проверяет шлюз.

import * as preact from 'preact';
import type { ComponentChildren } from 'preact';
import * as hooks from 'preact/hooks';
import * as jsxRuntime from 'preact/jsx-runtime';

import { api, can } from '../api';
import { packageSettings } from '../plugins/loader';
import { usePanel } from '../shell/context';
import { definePlugin } from '../shell/registry';
import { Action } from '../ui/cards';
import { ask } from '../ui/confirm';
import { Sheet } from '../ui/sheet';
import { Switch } from '../ui/switch';

/** Бэкенд своего плагина: `call('stations', { query })` — функция из contributes.panel.calls. */
export function useApi(pluginId: string): { call: <T = unknown>(fn: string, input?: unknown) => Promise<T> } {
    return hooks.useMemo(() => ({ call: <T,>(fn: string, input: unknown = {}) => api.callPackage<T>(pluginId, fn, input) }), [pluginId]);
}

/** Значения настроек своего плагина (меняет их администратор в окне плагина): только чтение. */
export function useSettings(pluginId: string): Record<string, unknown> {
    const panel = usePanel();
    return hooks.useMemo(() => ({ ...(packageSettings.get(pluginId) ?? {}) }), [pluginId, panel.version]);
}

/** Что плагину можно знать о пульте: номер перемены (перечитать своё) и переход в раздел. */
export function usePanelState(): { version: number; go: (section: string, tab?: string, item?: string) => void } {
    const panel = usePanel();
    return { version: panel.version, go: (section, tab, item) => panel.go(section, tab, item) };
}

/** Карточка «Стекла» с заголовком. */
export function Card({ title, meta, children }: { title?: string; meta?: string; children?: ComponentChildren }) {
    return (
        <section class="card glass card-list">
            {(title || meta) && (
                <div class="card-head">
                    {title && <h2 class="t-title-xs m-0">{title}</h2>}
                    {meta && <span class="t-meta">{meta}</span>}
                </div>
            )}
            {children}
        </section>
    );
}

/** Подпись: обычная, предупреждение или ошибка. */
export function Note({ tone = 'meta', children }: { tone?: 'meta' | 'warning' | 'error'; children?: ComponentChildren }) {
    const cls = tone === 'error' ? 'c-dot-offline' : tone === 'warning' ? 'c-pill-yandex-fg' : '';
    return <p class={`t-caption m-0 ${cls}`}>{children}</p>;
}

/** Метка: own — наше, yandex — Яндекс, agent — агент, off — нейтральная. */
export function Pill({ tone = 'off', children }: { tone?: 'own' | 'yandex' | 'agent' | 'off'; children?: ComponentChildren }) {
    return <span class={`pill pill-${tone}`}>{children}</span>;
}

export const sdk = { definePlugin, useApi, useSettings, usePanelState, can, Card, Action, Switch, Sheet, Note, Pill, ask };

declare global {
    // eslint-disable-next-line no-var
    var __duowave: { preact: typeof preact; hooks: typeof hooks; jsxRuntime: typeof jsxRuntime; sdk: typeof sdk } | undefined;
}

/** Отдать плагинам Preact и SDK пульта — до загрузки первого panel.js. */
export function installSdk(): void {
    globalThis.__duowave = { preact, hooks, jsxRuntime, sdk };
}
