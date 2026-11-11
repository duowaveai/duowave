// «Для разработчиков» (docs/plans/panel-rebuild.md, «Три слоя»): один переключатель в «Настройках» открывает третий
// слой — «Мастерскую» в доке, журнал запросов, таблицу «Кто отвечает», провайдеров и ключи. Помнится в этом браузере:
// это вид пульта, а не права — права по-прежнему у шлюза.

import { useEffect, useState } from 'preact/hooks';

const KEY = 'panel-dev';

let on = (() => {
    try {
        return localStorage.getItem(KEY) === '1';
    } catch {
        return false;
    }
})();

const listeners = new Set<() => void>();

export function devMode(): boolean {
    return on;
}

export function setDevMode(next: boolean): void {
    on = next;
    try {
        if (next) localStorage.setItem(KEY, '1');
        else localStorage.removeItem(KEY);
    } catch {
        // приватное окно: помним до перезагрузки
    }
    for (const listener of [...listeners]) listener();
}

/** Включён ли слой разработчика; перерисовка, когда его переключили. */
export function useDevMode(): boolean {
    const [, rerender] = useState(0);
    useEffect(() => {
        const listener = () => rerender((n) => n + 1);
        listeners.add(listener);
        return () => {
            listeners.delete(listener);
        };
    }, []);
    return on;
}
