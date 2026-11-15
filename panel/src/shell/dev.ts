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

// Скрытые возможности разработчика: восемь нажатий подряд на «Для разработчиков» в списке «Настроек» открывают их
// на странице этого раздела (первая — «Показывать скрытые пакеты»). Помнится в этом браузере, как и слой; сами
// возможности — настройки шлюза, их меняет администратор.
const HIDDEN_KEY = 'panel-dev-hidden';
/** Сколько нажатий открывают скрытые возможности и сколько ждать между ними. */
export const HIDDEN_TAPS = 8;
const TAP_GAP_MS = 1500;

let hidden = (() => {
    try {
        return localStorage.getItem(HIDDEN_KEY) === '1';
    } catch {
        return false;
    }
})();
let taps = 0;
let lastTap = 0;

export function hiddenDevOptions(): boolean {
    return hidden;
}

export function setHiddenDevOptions(next: boolean): void {
    hidden = next;
    try {
        if (next) localStorage.setItem(HIDDEN_KEY, '1');
        else localStorage.removeItem(HIDDEN_KEY);
    } catch {
        // приватное окно: помним до перезагрузки
    }
    for (const listener of [...listeners]) listener();
}

/**
 * Одно нажатие на «Для разработчиков»: восьмое подряд (не дольше полутора секунд между ними) открывает скрытые
 * возможности. Ответ — сколько ещё осталось (0 — открылись сейчас), или null, когда они уже открыты.
 */
export function tapDev(now = Date.now()): number | null {
    if (hidden) return null;
    taps = now - lastTap <= TAP_GAP_MS ? taps + 1 : 1;
    lastTap = now;
    if (taps < HIDDEN_TAPS) return HIDDEN_TAPS - taps;
    taps = 0;
    setHiddenDevOptions(true);
    return 0;
}

/** Открыты ли скрытые возможности; перерисовка, когда их открыли или закрыли. */
export function useHiddenDevOptions(): boolean {
    const [, rerender] = useState(0);
    useEffect(() => {
        const listener = () => rerender((n) => n + 1);
        listeners.add(listener);
        return () => {
            listeners.delete(listener);
        };
    }, []);
    return hidden;
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
