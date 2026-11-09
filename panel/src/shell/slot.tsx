// Слоты — места в экранах, куда плагины добавляют своё: карточки главного экрана (home.cards), кнопки шапки
// (header.actions), вкладки «Дома» (house.tabs), дополнения шита «Сказать» (say.extras). Каждый вклад живёт в своей
// границе ошибок: упавший прячется (и пишет в консоль, кто и почему), соседи и экран работают дальше.

import { Component, ComponentChildren, FunctionComponent } from 'preact';

import { can } from '../api';
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
    /** Для слотов-вкладок (house.tabs): подпись и значок вкладки. */
    title?: string;
    icon?: Icon;
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
