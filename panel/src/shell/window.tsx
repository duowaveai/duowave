// Окно раздела поверх стола (docs/plans/desk.md): заголовок, у помощника — чьё окно (полоса колонки или тёмная метка
// «Этот компьютер»), содержимое раздела и «Закрыть». Верхняя строка и док остаются видны; на телефоне окно — во весь
// экран. Esc закрывает окно, если его не забрало то, что внутри или поверх: поле ввода, список, меню, шит. Несохранённое
// внутри (мастерская) спрашивает раньше, чем окно закроется: закрытие — переход пульта (app.tsx, shell/leave.ts).

import { ComponentChildren, createContext } from 'preact';
import { useEffect, useRef, useState } from 'preact/hooks';

import { IconClose, IconLaptop } from '../icons';
import type { OpenWindow } from './shell';

/** Слово пульта колонки из фрейма окна помощника: Esc нажали там (shell/station-window.tsx). */
export const ESCAPE_MESSAGE = 'duowave:escape';

/**
 * Поверх окна открыто то, что закрывается своим Esc: шит, вопрос, «+», меню «Ещё», шит во фрейме окна колонки (его
 * затемнение на странице помощника — `.frame-scrim`).
 */
const ABOVE = '[aria-modal="true"], .tab-more, .dock-more, .frame-scrim';

/**
 * Окно над затемнением: шит во фрейме окна колонки у помощника (shell/station-window.tsx) затемняет всю страницу, а
 * окно встаёт над затемнением, чтобы шит во фрейме был виден. Заголовок и рамка окна при этом под вуалью, как остальная
 * страница; нажатие на них закрывает шит фрейма. Аргумент — как закрыть этот шит; null — шит закрыт. Вне окна стола
 * (боковая панель) контекста нет: фрейм встаёт над затемнением сам.
 */
export const WindowLift = createContext<((dismiss: (() => void) | null) => void) | null>(null);

/** Куда печатают: Esc там — отмена в поле, а не «закрыть окно». */
function typing(target: Element): boolean {
    if (target instanceof HTMLTextAreaElement || target instanceof HTMLSelectElement) return true;
    if (target instanceof HTMLInputElement) return !['checkbox', 'radio', 'button', 'submit', 'reset', 'range', 'color', 'file'].includes(target.type);
    return target instanceof HTMLElement && target.isContentEditable;
}

/** Esc ничей: его не забрали поле ввода, открытый список или меню, шит или вопрос поверх окна. */
export function escapeIsFree(event: KeyboardEvent): boolean {
    if (event.key !== 'Escape' || event.defaultPrevented) return false;
    const target = event.target instanceof Element ? event.target : null;
    if (target && (typing(target) || target.closest('[role="listbox"], [role="menu"], .menu'))) return false;
    return !document.querySelector(ABOVE);
}

export function DeskWindow({ win, strip }: { win: OpenWindow; strip?: ComponentChildren }) {
    const box = useRef<HTMLElement>(null);
    const close = useRef(win.onClose);
    close.current = win.onClose;
    /** Шит во фрейме окна открыт: как его закрыть нажатием на вуаль (WindowLift). */
    const [veil, setVeil] = useState<{ dismiss: () => void } | null>(null);
    const lift = useRef((dismiss: (() => void) | null) => setVeil(dismiss ? { dismiss } : null)).current;
    // Новое окно — фокус на него: клавиатура и чтец экрана начинают с его заголовка.
    useEffect(() => {
        box.current?.focus({ preventScroll: true });
    }, [win.id]);
    useEffect(() => {
        const onKey = (event: KeyboardEvent) => {
            if (!escapeIsFree(event)) return;
            event.preventDefault();
            close.current();
        };
        // Фокус во фрейме окна колонки: её Esc до этой страницы не доходит — пульт колонки присылает слово.
        const onMessage = (event: MessageEvent) => {
            if (event.origin !== location.origin || (event.data as { type?: unknown } | null)?.type !== ESCAPE_MESSAGE) return;
            const frames = [...(box.current?.querySelectorAll('iframe') ?? [])];
            if (!frames.some((frame) => frame.contentWindow === event.source) || document.querySelector(ABOVE)) return;
            close.current();
        };
        window.addEventListener('keydown', onKey);
        window.addEventListener('message', onMessage);
        return () => {
            window.removeEventListener('keydown', onKey);
            window.removeEventListener('message', onMessage);
        };
    }, []);
    return (
        <WindowLift.Provider value={lift}>
            <section
                ref={box}
                class={`desk-window${win.scope ? ` desk-window-${win.scope}` : ''}${veil ? ' is-lifted' : ''}`}
                aria-label={win.title}
                tabIndex={-1}
            >
                <header class="desk-window-head">
                    {strip}
                    {win.scope === 'own' && (
                        <span class="role-label">
                            <IconLaptop size={13} />
                            Этот компьютер
                        </span>
                    )}
                    <h1 class="desk-window-title">{win.title}</h1>
                    {win.badge}
                    <button class="desk-window-close" aria-label="Закрыть окно" onClick={win.onClose}>
                        <IconClose size={18} />
                    </button>
                </header>
                <div class="desk-window-body">{win.content}</div>
                {veil && <button class="desk-window-veil" aria-label="Закрыть окно поверх" tabIndex={-1} onClick={veil.dismiss} />}
            </section>
        </WindowLift.Provider>
    );
}
