// Что знают о MagicPlace точка в доке и виджет стола, не открывая его (docs/plans/magicplace.md): одно чтение каталога
// на весь пульт — сколько обновлений, что новое и что рекомендуем. Перечитывается, когда шлюз говорит о переменах
// пакетов (`packages.changed`, `packages.catalog`), и не чаще раза в пять минут иначе: открытое окно MagicPlace читает
// каталог само.

import { useEffect, useState } from 'preact/hooks';

import { CatalogView, api, can } from '../api';

/** Перечитать не раньше, чем через столько, если шлюз ничего не сказал. */
const STALE_MS = 5 * 60_000;

let view: CatalogView | null = null;
let readAt = 0;
let reading: Promise<void> | null = null;
const listeners = new Set<(next: CatalogView | null) => void>();

function tell(): void {
    for (const listener of [...listeners]) listener(view);
}

async function read(): Promise<void> {
    if (reading) return reading;
    if (!can('station.view')) return;
    reading = (async () => {
        try {
            view = await api.packagesCatalog();
            tell();
        } catch {
            // Шлюз постарше или без сети — точки и строк нет, MagicPlace откроется и скажет сам.
        } finally {
            readAt = Date.now();
            reading = null;
        }
    })();
    return reading;
}

/** Шлюз сказал, что пакеты поменялись, или окно MagicPlace перечитало каталог: снимок — заново. */
export function noteStoreChange(name: string, fresh?: CatalogView): void {
    if (fresh) {
        view = fresh;
        readAt = Date.now();
        tell();
        return;
    }
    if (name === 'packages.changed' || name === 'packages.catalog' || name === 'hello') {
        readAt = 0;
        if (listeners.size > 0) void read();
    }
}

/** Снимок каталога для точки и виджета: null — ещё не прочитан или не прочитать. */
export function useStoreView(): CatalogView | null {
    const [seen, setSeen] = useState<CatalogView | null>(view);
    useEffect(() => {
        listeners.add(setSeen);
        if (Date.now() - readAt > STALE_MS) void read();
        else setSeen(view);
        return () => {
            listeners.delete(setSeen);
        };
    }, []);
    return seen;
}
