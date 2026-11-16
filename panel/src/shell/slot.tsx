// Слоты — места в экранах, куда плагины добавляют своё: карточки главного экрана (home.cards), кнопки шапки
// (header.actions), вкладки «Дома» (house.tabs), дополнения шита «Сказать» (say.extras). Каждый вклад живёт в своей
// границе ошибок: упавший прячется (и пишет в консоль, кто и почему), соседи и экран работают дальше.

import { Component, ComponentChildren, FunctionComponent } from 'preact';

import { can } from '../api';
import type { Health } from '../api';
import { registeredPlugins, useRegistry } from './registry';
import type { Icon } from './sections';

/** Вклад плагина в слот: карточка на главном экране, кнопка в шапке, вкладка «Дома»… */
export interface SlotContribution {
    id: string;
    Component: FunctionComponent<Record<string, unknown>>;
    /** Порядок среди вкладов слота: меньше — раньше. */
    order?: number;
    /** Право из /auth/me, без которого вклада не видно. */
    permission?: string;
    /** Для слотов-вкладок (house.tabs) и виджетов стола (desk.widgets): подпись и значок. */
    title?: string;
    icon?: Icon;
    /** Виджет стола: маленький (в ряд по два на телефоне) или большой (во всю ширину). По умолчанию маленький. */
    size?: 'small' | 'large';
    /** Виджет стола: подпись источника в его шапке — «колонка», «журнал», «плагин»… */
    source?: string;
    /**
     * Виджет стола: стоял сразу на прежнем стартовом столе (до docs/plans/start-desk.md). Так он и стоит у столов, которые
     * человек менял раньше: убранное там не появляется, стоявшее не пропадает. Без него — только из «+».
     */
    initial?: boolean;
    /**
     * Виджет стола стоит сразу на стартовом столе (docs/plans/start-desk.md): сцена, витрина и живые виджеты, которые
     * прячутся, пока пусты. Только у встроенных: приложения сами на стол не встают.
     */
    start?: boolean;
    /** Виджет стола: для каких ролей пульта ('gateway', 'helper'); без него — для обеих. */
    roles?: Array<'gateway' | 'helper'>;
    /** Виджет стола рисует свою рамку сам (карточку плагина, сцену). */
    bare?: boolean;
    /**
     * Виджет стола стоит прямо под сценой — колонкой или ДиВи — во всю её ширину, пока человек его не передвинул: так
     * главное приложение дома («Лента») видно сразу, без поисков. На телефоне — первым после сцены.
     */
    under?: boolean;
    /** Виджет под сценой шире сцены — во всю ширину витрины (docs/plans/start-desk.md). */
    wide?: boolean;
    /** Виджет стола: есть ли ему что показать сейчас (по /health); нет — его нет ни на столе, ни в «+». */
    available?: (health: Health | null) => boolean;
    /**
     * Виджет стола стоит на столе, но не виден, пока ему нечего показать: сам говорит, пуст ли он (`useWidgetEmpty`,
     * ui/widget.tsx). Так у новичка нет пустых «Последних вопросов» и «Работ» — они появляются с первым вопросом и первой
     * работой. Пока не сказал — не виден.
     */
    hideWhileEmpty?: boolean;
}

/** Вклад с плагином, который его принёс: ключ `плагин.вклад` уникален во всём пульте. */
export interface PlacedContribution extends SlotContribution {
    plugin: string;
    key: string;
}

/** Вклады в слот, которые видит этот вход, по порядку; перерисовка — когда реестр меняется. */
export function useSlot(name: string): PlacedContribution[] {
    useRegistry();
    const placed: PlacedContribution[] = [];
    for (const plugin of registeredPlugins())
        for (const contribution of plugin.slots?.[name] ?? [])
            placed.push({ ...contribution, plugin: plugin.id, key: `${plugin.id}.${contribution.id}` });
    return placed.filter((entry) => !entry.permission || can(entry.permission)).sort((a, b) => (a.order ?? 0) - (b.order ?? 0));
}

interface BoundaryProps {
    /** Чей это вклад — для консоли. */
    name: string;
    /** Что показать вместо упавшего; по умолчанию — ничего. */
    fallback?: ComponentChildren;
    children: ComponentChildren;
}

/** Граница ошибок: упавшее при отрисовке или в эффектах заменяется на fallback (по умолчанию — пусто). */
export class Boundary extends Component<BoundaryProps, { failed: boolean }> {
    state = { failed: false };

    static getDerivedStateFromError(): { failed: boolean } {
        return { failed: true };
    }

    componentDidCatch(error: unknown): void {
        console.error(`Пульт: «${this.props.name}» упал и скрыт`, error);
    }

    render() {
        return this.state.failed ? (this.props.fallback ?? null) : this.props.children;
    }
}

/** Один вклад в своей границе ошибок; упавший — пусто или fallback (вкладке нужно хоть что-то сказать). */
export function Contribution({ entry, props, fallback }: { entry: PlacedContribution; props?: Record<string, unknown>; fallback?: ComponentChildren }) {
    const { Component: Body } = entry;
    return (
        <Boundary name={entry.key} fallback={fallback}>
            <Body {...props} />
        </Boundary>
    );
}

/** Все вклады в слот по порядку, каждый в своей границе. Пустой слот ничего не рисует — раскладка не меняется. */
export function Slot({ name, props }: { name: string; props?: Record<string, unknown> }) {
    const entries = useSlot(name);
    if (entries.length === 0) return null;
    return (
        <>
            {entries.map((entry) => (
                <Contribution key={entry.key} entry={entry} props={props} />
            ))}
        </>
    );
}
