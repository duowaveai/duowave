// «Для разработчиков» (docs/plans/panel-rebuild.md, «Три слоя»): один переключатель в «Настройках» открывает третий
// слой — технические строки на своих местах: журнал запросов, таблицу «Кто отвечает» подробно, провайдеров и ключи
// (docs/plans/menu-scheme.md: ни кнопок в доке, ни списка ярлыков). Это вид пульта, а не права — права по-прежнему у
// шлюза. Помнится у человека на шлюзе (shell/memory.ts): включил на компьютере — включено и на телефоне.

import { useEffect, useState } from 'preact/hooks';

import { onPanelMemory, recalled, remember } from './memory';

/** Ключи памяти пульта. */
const DEV = 'dev';
const HIDDEN = 'devHidden';

// До памяти на шлюзе оба помнились только в этом браузере: переносим один раз.
for (const [legacy, key] of [
    ['panel-dev', DEV],
    ['panel-dev-hidden', HIDDEN],
] as const) {
    try {
        if (localStorage.getItem(legacy) === '1' && recalled(key) === undefined) remember(key, true);
        localStorage.removeItem(legacy);
    } catch {
        // приватное окно: переносить нечего
    }
}

export function devMode(): boolean {
    return recalled<boolean>(DEV) === true;
}

export function setDevMode(next: boolean): void {
    remember(DEV, next ? true : undefined);
}

// Скрытые возможности разработчика: восемь нажатий подряд на «Для разработчиков» в списке «Настроек» открывают их
// на странице этого раздела (первая — «Показывать скрытые пакеты»). Помнятся у человека, как и слой; сами
// возможности — настройки шлюза, их меняет администратор.
/** Сколько нажатий открывают скрытые возможности и сколько ждать между ними. */
export const HIDDEN_TAPS = 8;
const TAP_GAP_MS = 1500;

let taps = 0;
let lastTap = 0;

export function hiddenDevOptions(): boolean {
    return recalled<boolean>(HIDDEN) === true;
}

export function setHiddenDevOptions(next: boolean): void {
    remember(HIDDEN, next ? true : undefined);
}

/**
 * Одно нажатие на «Для разработчиков»: восьмое подряд (не дольше полутора секунд между ними) открывает скрытые
 * возможности. Ответ — сколько ещё осталось (0 — открылись сейчас), или null, когда они уже открыты.
 */
export function tapDev(now = Date.now()): number | null {
    if (hiddenDevOptions()) return null;
    taps = now - lastTap <= TAP_GAP_MS ? taps + 1 : 1;
    lastTap = now;
    if (taps < HIDDEN_TAPS) return HIDDEN_TAPS - taps;
    taps = 0;
    setHiddenDevOptions(true);
    return 0;
}

/** Перерисовка, когда память пульта поменялась (здесь, после входа или с другого устройства). */
function useMemoryTick(): void {
    const [, rerender] = useState(0);
    useEffect(() => onPanelMemory(() => rerender((n) => n + 1)), []);
}

/** Открыты ли скрытые возможности; перерисовка, когда их открыли или закрыли. */
export function useHiddenDevOptions(): boolean {
    useMemoryTick();
    return hiddenDevOptions();
}

/** Включён ли слой разработчика; перерисовка, когда его переключили. */
export function useDevMode(): boolean {
    useMemoryTick();
    return devMode();
}
