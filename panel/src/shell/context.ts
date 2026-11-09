// Что пульт знает и что умеет — для экранов разделов: данные шлюза, выбранная колонка и переходы между разделами.
// Разделы получают это из контекста, а не пропсами: оболочка рисует любой раздел одинаково, не зная его данных.

import { ComponentChildren, createContext } from 'preact';
import { useContext } from 'preact/hooks';

import type { AccessCatalog, ApiError, Health, RequestView, SessionSnapshot, Station, UnsupportedEntry } from '../api';
import type { LiveState } from '../live';
import type { StationDrawing } from '../stations';
import type { Place } from './links';

export interface DebugState {
    sessions: SessionSnapshot[] | null;
    unsupported: UnsupportedEntry[];
    error: ApiError | null;
}

/**
 * Переходы: раздел, вкладка внутри него и то, что во вкладке открыто. Раздел помнит, где его оставили; ссылка может
 * вести сразу на вкладку и на её элемент. Каждый переход пишет адрес (`#/<раздел>/<вкладка>/<id>`, shell/links.ts).
 */
export interface Navigation {
    /** Открытый раздел. */
    section: string;
    /** В раздел; названа вкладка — сразу на неё, назван элемент — сразу к нему (лист, карточка). */
    go: (section: string, tab?: string, item?: string, query?: Record<string, string>) => void;
    /** Вкладка, на которой раздел оставили, иначе fallback. */
    tabOf: <T extends string>(section: string, fallback: T) => T;
    setTab: (section: string, tab: string) => void;
    /** Где пульт сейчас по адресу: раздел, вкладка, открытый элемент и параметры ссылки. */
    link: Place;
    /**
     * Открыть элемент вкладки (null — закрыть): адрес следует за ним. replace — без нового шага истории (снять
     * разовый параметр вроде `?install`, закрыть открытое ссылкой).
     */
    openItem: (section: string, tab: string, item: string | null, query?: Record<string, string>, replace?: boolean) => void;
}

export interface PanelState extends Navigation {
    health: Health | null;
    offline: boolean;
    debug: DebugState;
    catalog: AccessCatalog | null;
    /** Колонки дома из записей шлюза; null — список не прочитать (нет права или шлюз постарше). */
    stations: Station[] | null;
    feed: LiveState;
    /** Выбранная колонка, её сессия и рисунок; на связи ли она. */
    current: Station | null;
    session?: SessionSnapshot;
    drawing: StationDrawing;
    online: boolean;
    /** Путь вопроса на дашборде: идущий запрос колонки, иначе последний законченный. */
    trip?: RequestView;
    /** Растёт с каждой переменой на шлюзе: дашборд перечитывает своё. */
    version: number;
    /** Последние запросы колонки (только тому, кто может читать журнал). */
    recent?: RequestView[];
    /** Плашки над режимом на главном экране: новая колонка, новая версия шлюза. */
    banner: ComponentChildren;
    /** Колонок ещё нет (список прочитан, и он пуст): главный экран зовёт подключить первую. */
    noStation: boolean;
    /** Открыть мастер «Подключить колонку». */
    openConnect: () => void;
    /** В «Дом» → «Коннекторы», сразу в лист этого подключения, если оно названо. */
    openHouse: (connector?: string) => void;
    /** Лист подключения, который «Дом» откроет сразу (ссылка с дашборда), и отметка, что открыл. */
    openConnector: string | null;
    connectorOpened: () => void;
    signOut: () => void;
}

export const PanelContext = createContext<PanelState | null>(null);

/** Данные и переходы пульта для экрана раздела. */
export function usePanel(): PanelState {
    const panel = useContext(PanelContext);
    if (!panel) throw new Error('usePanel: экран раздела рисуется вне пульта');
    return panel;
}

/** Элемент вкладки, открытый по адресу: id и параметры ссылки, открыть и закрыть. */
export interface LinkedItem {
    /** id из адреса; null — во вкладке ничего не открыто. */
    item: string | null;
    query: Record<string, string>;
    open: (item: string, query?: Record<string, string>) => void;
    /** Закрыть: адрес снова ведёт на вкладку. */
    close: () => void;
    /** Оставить элемент, но снять параметры ссылки (разовое `?install` сделано). */
    settle: () => void;
}

/** Что во вкладке раздела открыто по ссылке (`#/<раздел>/<вкладка>/<id>?…`) — и как открыть и закрыть своё. */
export function useLinkedItem(section: string, tab: string): LinkedItem {
    const panel = usePanel();
    const here = panel.link.section === section && panel.link.tab === tab;
    const item = here ? (panel.link.item ?? null) : null;
    return {
        item,
        query: here ? panel.link.query : {},
        open: (next, query) => panel.openItem(section, tab, next, query),
        close: () => panel.openItem(section, tab, null),
        settle: () => item && panel.openItem(section, tab, item, {}, true),
    };
}

/** Вкладка внутри раздела, которую раздел помнит: [вкладка, сменить]. */
export function useSectionTab<T extends string>(section: string, fallback: T): [T, (tab: T) => void] {
    const panel = usePanel();
    return [panel.tabOf(section, fallback), (tab: T) => panel.setTab(section, tab)];
}
