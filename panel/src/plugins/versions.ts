// Номер перемены плагина (docs/plans/desk-after-station.md, Б): `usePanelState().version` в виджете плагина растёт
// только с переменами его пакета — `packages.data` (его данные) и `packages.changed` (включили, обновили, настройки) с
// его id — и с переменой пакетов без id (шлюз постарше) или с новым hello канала. Раньше он рос с каждой переменой в
// пульте, и «Погода» на колонке за 40 минут получила 368 вызовов. Не чаще раза в 5 с на плагин: голос пишет список
// пачкой, а виджет перечитывает раз. Код плагина загрузился или выгрузился — данные те же: не перечитывать.

import { createContext } from 'preact';
import { useEffect, useState } from 'preact/hooks';

/** Чей это компонент: id пакета, чей раздел или вклад рисуется (plugins/loader.ts кладёт его вокруг каждого). */
export const PluginScope = createContext<string | null>(null);

/** Перечитывать не чаще. */
const SETTLE_MS = 5000;
/** Перемены пакета, после которых его данные те же: код загрузился, выгрузился, не загрузился; памяти мало; поменялось
 *  расписание (поставили, сработало, пауза — plugins/schedule.rs). */
const RUNTIME = new Set(['loaded', 'unloaded', 'failed', 'pressure', 'schedule']);
/** Перемены, о которых плагину перечитать своё. */
const OF_PACKAGES = new Set(['packages.changed', 'packages.data']);

/** Перемена пакета только в работе его кода (`change` у `packages.changed`): ни данных, ни частей пульта она не меняет. */
export function runtimeOnly(change?: string): boolean {
    return !!change && RUNTIME.has(change);
}

const versions = new Map<string, number>();
const lastAt = new Map<string, number>();
const timers = new Map<string, number>();
/** Плагины, чьи компоненты сейчас на экране: им и перечитывать. */
const listeners = new Map<string, Set<() => void>>();

function apply(id: string): void {
    versions.set(id, (versions.get(id) ?? 0) + 1);
    lastAt.set(id, Date.now());
    for (const listener of [...(listeners.get(id) ?? [])]) listener();
}

/** Перечитать: сейчас или, если недавно перечитывал, — когда пройдут 5 с. Несколько перемен подряд — одно чтение. */
function bump(id: string): void {
    if (timers.has(id)) return;
    const wait = (lastAt.get(id) ?? 0) + SETTLE_MS - Date.now();
    if (wait <= 0) return apply(id);
    timers.set(
        id,
        window.setTimeout(() => {
            timers.delete(id);
            apply(id);
        }, wait),
    );
}

/**
 * Перемена, о которой сказал живой канал (свой или колонки помощника): кому из плагинов на экране перечитать своё.
 * hello — канал открылся заново: перечитать всем, кто не читал только что.
 */
export function notePackageChange(name: string, subject?: string, change?: string): void {
    if (name === 'hello') {
        for (const id of listeners.keys()) if (Date.now() - (lastAt.get(id) ?? 0) >= SETTLE_MS) bump(id);
        return;
    }
    if (!OF_PACKAGES.has(name) || runtimeOnly(change)) return;
    if (subject) bump(subject);
    else for (const id of listeners.keys()) bump(id);
}

/** Номер перемены плагина `id` (null — не плагин: 0). Компонент перерисуется, когда он вырастет. */
export function usePluginVersion(id: string | null): number {
    const [, rerender] = useState(0);
    useEffect(() => {
        if (!id) return;
        // Только что появился на экране — только что прочитал своё.
        lastAt.set(id, Date.now());
        const update = (): void => rerender((n) => n + 1);
        const set = listeners.get(id) ?? new Set<() => void>();
        set.add(update);
        listeners.set(id, set);
        return () => {
            set.delete(update);
            if (set.size === 0) listeners.delete(id);
        };
    }, [id]);
    return id ? (versions.get(id) ?? 0) : 0;
}
