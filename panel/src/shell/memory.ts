// Что пульт помнит о человеке (docs/plans/menu-scheme.md): где открыты «Папки», включён ли слой разработчика. Дом этого —
// шлюз (`/api/v1/panel/state`, gateway/src/panel/state.rs): телефон и компьютер одного человека видят одно и то же.
// Копия в браузере — чтобы пульт при запуске сразу был таким, каким его оставили, и чтобы помнить без входа (приватное
// окно, ключ). Что принадлежит устройству — тема, язык, микрофон — сюда не попадает.

import { useEffect, useState } from 'preact/hooks';

import { api } from '../api';
import { VIA } from '../via';

/**
 * Копия — у каждого шлюза своя: окно колонки у помощника (via.ts) открыто с адреса компьютера, и общая копия носила
 * память колонки в пульт компьютера и обратно — так запись «Первых шагов» компьютера оказывалась у колонки.
 */
const COPY = VIA ? `panel-memory@${VIA}` : 'panel-memory';
/** Правки подряд (листают папки) уходят на шлюз одной записью — когда человек остановился. */
const SAVE_AFTER = 800;

type Memory = Record<string, unknown>;

let memory: Memory = (() => {
    try {
        const kept = JSON.parse(localStorage.getItem(COPY) ?? 'null');
        return kept && typeof kept === 'object' && !Array.isArray(kept) ? (kept as Memory) : {};
    } catch {
        return {};
    }
})();

const listeners = new Set<() => void>();
/** Ключи, что ждут отправки на шлюз: значение или null («забыть»). */
let pending: Memory = {};
let timer: number | undefined;

function keepCopy(): void {
    try {
        localStorage.setItem(COPY, JSON.stringify(memory));
    } catch {
        // приватное окно: помним до перезагрузки
    }
}

function notify(): void {
    for (const listener of [...listeners]) listener();
}

function send(): void {
    void flushPanelMemory();
}

/**
 * Отправить ждущее на шлюз сейчас, не дожидаясь паузы: перед уходом со страницы (окно помощника уходит в пульт колонки,
 * app.tsx) — иначе правка последних 800 мс осталась бы только в копии браузера. Без входа (или ключ) шлюз не помнит —
 * остаётся копия; ошибка не мешает уйти. Небольшая правка идёт с keepalive — доходит, даже если страницу тут же
 * перезагрузили.
 */
export function flushPanelMemory(): Promise<void> {
    window.clearTimeout(timer);
    timer = undefined;
    const changes = pending;
    pending = {};
    if (Object.keys(changes).length === 0) return Promise.resolve();
    // keepalive берёт тело до 64 КБ, больше — запрос не уходит вовсе: большая правка идёт обычным запросом.
    const body = JSON.stringify(changes).length;
    return api.rememberPanel(changes, body < 60_000).then(
        () => undefined,
        () => undefined,
    );
}

// Страницу закрыли или перезагрузили в паузе перед отправкой: ждущее уходит сейчас — иначе при новом запуске шлюз
// вернул бы старое (так «Первые шаги» открывались снова после × и быстрой перезагрузки).
if (typeof window !== 'undefined') window.addEventListener('pagehide', () => void flushPanelMemory());

/** Что помнится под ключом. */
export function recalled<T>(key: string): T | undefined {
    return memory[key] as T | undefined;
}

/** Запомнить под ключом (undefined — забыть): сразу здесь, на шлюз — чуть позже, одной записью. */
export function remember(key: string, value: unknown): void {
    if (JSON.stringify(memory[key]) === JSON.stringify(value)) return;
    if (value === undefined) delete memory[key];
    else memory[key] = value;
    keepCopy();
    notify();
    pending[key] = value === undefined ? null : value;
    window.clearTimeout(timer);
    timer = window.setTimeout(send, SAVE_AFTER);
}

/**
 * После входа: память со шлюза — главная. Ключи, которых шлюз ещё не знает (помнилось только в этом браузере, до этой
 * версии), уходят на шлюз, чтобы и другие устройства их узнали.
 */
export async function loadPanelMemory(): Promise<void> {
    let kept: Memory;
    try {
        kept = (await api.panelMemory()).state ?? {};
    } catch {
        return;
    }
    const onlyHere = Object.fromEntries(Object.entries(memory).filter(([key]) => !(key in kept)));
    memory = { ...kept, ...onlyHere, ...pending };
    keepCopy();
    notify();
    if (Object.keys(onlyHere).length > 0) {
        pending = { ...onlyHere, ...pending };
        window.clearTimeout(timer);
        timer = window.setTimeout(send, SAVE_AFTER);
    }
}

/** Перечитать с шлюза, когда другой браузер что-то поменял — или просто при возвращении на вкладку. */
export function onPanelMemory(listener: () => void): () => void {
    listeners.add(listener);
    return () => listeners.delete(listener);
}

/** То же для компонентов: значение ключа и перерисовка, когда оно меняется. */
export function useRecalled<T>(key: string): T | undefined {
    const [, setTick] = useState(0);
    useEffect(() => onPanelMemory(() => setTick((tick) => tick + 1)), []);
    return recalled<T>(key);
}
