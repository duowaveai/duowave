// SDK пульта для плагинов — типы для авторов panel.js (docs/plans/plugins.md, этап 5; реализация —
// panel/src/sdk/index.tsx). Модуль плагина — ES-модуль, который пульт загружает с шлюза
// (/panel-plugins/<id>/<версия>/panel.js), пока плагин включён. «preact», «preact/hooks», «preact/jsx-runtime» и
// «@duowave/panel-sdk» ведут к копиям пульта (import map): свою копию Preact плагин не везёт.
//
//   import { h } from "preact";
//   import { useEffect, useState } from "preact/hooks";
//   import { definePlugin, useApi, Card, Note } from "@duowave/panel-sdk";
//
//   function Stations() {
//     const api = useApi("radio");                         // id своего пакета
//     const [data, setData] = useState(null);
//     useEffect(() => { api.call("panelStations").then(setData); }, [api]);
//     return h(Card, { title: "Радио" }, h(Note, null, data ? `${data.stations.length} станций` : "…"));
//   }
//   export default definePlugin({ id: "radio", slots: { "home.cards": [{ id: "stations", Component: Stations }] } });
//
// В манифесте: "contributes": { "panel": { "slots": ["home.cards"], "calls": ["panelStations"] } } — calls: функции
// backend.js, которые пульт плагина может звать (POST /api/v1/packages/<id>/call/<функция>, право use:plugins).
// Стили — panel.css рядом, только через токены темы (var(--accent), var(--text) …). `?safe=1` в адресе пульта — без
// плагинов.

declare module "@duowave/panel-sdk" {
    import type { ComponentChildren, FunctionComponent } from "preact";

    /** Значок раздела или вкладки: компонент Preact со свойством size. */
    export type Icon = FunctionComponent<{ size?: number }>;

    /** Раздел пульта: своя страница в навигации. */
    export interface SectionDef {
        id: string;
        title: string;
        icon: Icon;
        Screen: FunctionComponent;
        /** Право из /auth/me, без которого раздела не видно. */
        permission?: string;
        /** Порядок в навигации: меньше — раньше. */
        order?: number;
        /** main — на панели вкладок, more — в «Ещё». */
        nav?: "main" | "more";
    }

    /**
     * Вклад в слот: desk.widgets — виджеты стола (docs/plans/desk.md), home.cards — карточка на столе большим
     * виджетом в своей рамке, header.actions — кнопки шапки, house.tabs — вкладки «Дома», say.extras — дополнения
     * окна «Сказать».
     *
     *   slots: { "desk.widgets": [{ id: "now", title: "Погода", icon: icons.weather, size: "small", initial: true,
     *                               source: "плагин", Component: Weather }] }
     *
     * Виджет рисует только содержимое: рамку с названием, значком, источником и меню «…» даёт стол. Крупное число —
     * класс `w-big`, строки — `w-rows` > `w-row`, подпись — `w-cap`, плитка дня — `w-day`, кнопка — `w-btn`.
     */
    export interface SlotContribution {
        id: string;
        Component: FunctionComponent<Record<string, unknown>>;
        order?: number;
        permission?: string;
        /** Для house.tabs и desk.widgets: подпись и значок. */
        title?: string;
        icon?: Icon;
        /** desk.widgets: маленький (на телефоне по два в ряд) или большой (во всю ширину); по умолчанию маленький. */
        size?: "small" | "large";
        /** desk.widgets: подпись источника в шапке — «плагин», «колонка»… */
        source?: string;
        /** desk.widgets: стоит на столе сразу; без него — его ставят из «+». */
        initial?: boolean;
        /** desk.widgets: для каких ролей пульта; без него — для обеих. */
        roles?: Array<"gateway" | "helper">;
        /** desk.widgets: виджет рисует свою рамку сам (класс `widget` на корне). */
        bare?: boolean;
    }

    /** Тема — данные (docs/design/glass/THEMES.md): чего нет, берётся у «Минимала». */
    export interface Theme {
        id: string;
        title: string;
        about?: string;
        modes?: { dark: string; light: string };
        palette: { dark: Record<string, string>; light: Record<string, string> };
        /** Вид, который тема советует: стол и док или боковая панель ('tabbar' — стол); выбирает человек. */
        layout?: "desk" | "tabbar" | "sidebar";
        density?: number;
        font?: string;
        radius?: { panel: number; large: number; bar: number };
    }

    export interface PanelPlugin {
        id: string;
        sections?: SectionDef[];
        slots?: Record<string, SlotContribution[]>;
        themes?: Theme[];
    }

    /** Описание плагина: модуль отдаёт его по умолчанию. Регистрирует пульт — под id пакета. */
    export function definePlugin(plugin: PanelPlugin): PanelPlugin;

    /** Бэкенд своего плагина: функции из contributes.panel.calls; права проверяет шлюз. */
    export function useApi(pluginId: string): { call<T = unknown>(fn: string, input?: unknown): Promise<T> };

    /** Значения настроек своего плагина (меняет их администратор в окне плагина). */
    export function useSettings(pluginId: string): Record<string, unknown>;

    /**
     * Номер перемены своего плагина (перечитать своё) и переход в раздел пульта. Растёт с переменами его пакета — его
     * данные (`host.kv`), настройки, включение — не чаще раза в 5 с; не с каждой переменой в пульте.
     */
    export function usePanelState(): { version: number; go(section: string, tab?: string, item?: string): void };

    /** Есть ли у вошедшего право. */
    export function can(permission: string): boolean;

    /** Запрос к колонке из живого канала: от первого звука до последнего слова ответа. Слова — тому, у кого журнал. */
    export interface RequestView {
        id: string;
        device: string;
        kind: "voice" | "text" | "say";
        status: "active" | "answered" | "silent" | "cancelled" | "failed";
        startedAt: string;
        ms?: number;
        text?: string;
        answer?: string;
        capability?: string;
    }

    /** Живой канал пульта, только чтение: открыт ли он, запросы к колонкам (новые сначала), как колонки слышно. */
    export function useLive(): {
        connected: boolean;
        requests: RequestView[];
        presence: Record<string, { device: string; state: "voice" | "alive" | "reconnecting" | "offline"; since: string }>;
    };

    /** Выбранная колонка, только чтение: устройство, имя, на связи ли, режим и роль пульта. */
    export function useStation(): {
        device: string | null;
        name: string;
        online: boolean;
        mode: "relay" | "gateway" | "own" | null;
        role: "gateway" | "helper";
    };

    /** Значки пульта: для шапки виджета (icon) и раздела. */
    export const icons: Record<"weather" | "music" | "timer" | "list" | "coins" | "globe" | "heart" | "speaker" | "sparkle" | "package", Icon>;

    export const Card: FunctionComponent<{ title?: string; meta?: string; children?: ComponentChildren }>;
    export const Note: FunctionComponent<{ tone?: "meta" | "warning" | "error"; children?: ComponentChildren }>;
    export const Pill: FunctionComponent<{ tone?: "own" | "yandex" | "agent" | "off"; children?: ComponentChildren }>;
    export const Action: FunctionComponent<{
        label: string;
        tone?: "off" | "on" | "yandex";
        busy?: boolean;
        disabled?: boolean;
        onClick: () => void;
    }>;
    export const Switch: FunctionComponent<{ on: boolean; label: string; disabled?: boolean; onChange: (on: boolean) => void }>;
    export const Sheet: FunctionComponent<{ title: string; about?: string; onClose: () => void; children?: ComponentChildren }>;
    /**
     * A question before something that cannot be undone, in the panel's own sheet (window.confirm shows nothing in the
     * desktop app): true for the confirming button, false for «Отмена», Esc or a tap outside. `danger` paints the button
     * red and puts the focus on «Отмена». Example: `if (!(await ask({ title: 'Удалить?', ok: 'Удалить', danger: true }))) return;`
     */
    export function ask(options: string | { title: string; text?: string; ok?: string; cancel?: string; danger?: boolean }): Promise<boolean>;
}
