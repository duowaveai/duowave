// Выбор темы, режима и вида: что выбрано, применение к странице, память в этом браузере. Сами темы — данные (themes.ts),
// приходят через реестр (definePlugin). Вид — стол и док или боковая панель — выбирается отдельно от темы
// (docs/plans/desk.md). Первую отрисовку до скриптов делает index.html: он читает тот же выбор.

import { useEffect, useState } from 'preact/hooks';

import { VIA } from '../via';
import { onRegistryChange, registeredPlugins } from './registry';
import { DEFAULT_THEME, Mode, Theme, activeCss } from './themes';

/** Выбор человека: { v: 2, id, mode } — null там, где он не выбирал и действует тема или режим по умолчанию. */
const CHOICE_KEY = 'panel-theme';
/** Прежний ключ пульта — только режим. Пишется для старых сборок. */
const LEGACY_KEY = 'glass-theme';
/** CSS последней применённой темы: index.html вставляет его до загрузки, и выбранная тема не мигает. */
const CSS_KEY = 'panel-theme-css';
/** Вид пульта: 'desk' — стол и док, 'sidebar' — боковая панель. */
const VIEW_KEY = 'panel-view';

export type View = 'desk' | 'sidebar';

/**
 * Темы, которых больше нет (10.11.2026): «Стекло» и «Простор» удалены, «Минимал · панель» стал видом. Их выбор — тема
 * по умолчанию, а у тех, что были с боковой панелью, — ещё и вид «Боковая панель», если вид не выбран.
 */
const RETIRED: Record<string, View> = { glass: 'desk', prostor: 'sidebar', 'minimal-panel': 'sidebar' };

interface Choice {
    /** Тема, которую выбрал человек; null — тема по умолчанию. */
    id: string | null;
    /** Режим, который выбрал человек; null — режим темы, а у темы без своего — как у системы. */
    mode: Mode | null;
}

function read(key: string): string | null {
    try {
        return localStorage.getItem(key);
    } catch {
        return null;
    }
}

function store(key: string, value: string): void {
    try {
        localStorage.setItem(key, value);
    } catch {
        // приватное окно: выбор живёт до перезагрузки
    }
}

/**
 * Выбор, с которым страница уже отрисована: index.html прочёл его до загрузки пульта (data-theme-pick и
 * data-mode-pick на <html>). Выбор удалённой темы — тема по умолчанию.
 */
function initialChoice(): Choice {
    const root = document.documentElement;
    const mode = root.dataset.modePick;
    const id = root.dataset.themePick || null;
    return { id: id && !(id in RETIRED) ? id : null, mode: mode === 'light' || mode === 'dark' ? mode : null };
}

/** Вид: выбранный, иначе — тот, что был у удалённой темы человека, иначе стол. */
function initialView(): View {
    const picked = read(VIEW_KEY);
    if (picked === 'desk' || picked === 'sidebar') return picked;
    try {
        const raw = JSON.parse(read(CHOICE_KEY) ?? 'null') as { id?: unknown } | null;
        if (raw && typeof raw.id === 'string' && raw.id in RETIRED) return RETIRED[raw.id];
    } catch {
        // чужая запись — вид по умолчанию
    }
    return 'desk';
}

let choice: Choice = initialChoice();
let view: View = initialView();
const listeners = new Set<() => void>();

function systemMode(): Mode {
    try {
        return matchMedia('(prefers-color-scheme: light)').matches ? 'light' : 'dark';
    } catch {
        return 'dark';
    }
}

/** Режим на странице: выбранный, иначе режим темы, иначе как у системы. */
function modeOf(theme: Theme): Mode {
    return choice.mode ?? theme.defaultMode ?? systemMode();
}

/** Темы всех включённых плагинов; встроенная — первой. Тема с тем же id из плагина позже заменяет прежнюю. */
export function availableThemes(): Theme[] {
    const byId = new Map<string, Theme>();
    for (const plugin of registeredPlugins()) for (const theme of plugin.themes ?? []) byId.set(theme.id, theme);
    if (!byId.has(DEFAULT_THEME.id)) byId.set(DEFAULT_THEME.id, DEFAULT_THEME);
    return [...byId.values()];
}

/** Тема, которая сейчас на странице: выбранная, а если её нет (плагин выключен или ещё не загружен) — по умолчанию. */
export function currentTheme(): Theme {
    const id = choice.id ?? DEFAULT_THEME.id;
    return availableThemes().find((theme) => theme.id === id) ?? DEFAULT_THEME;
}

/** Вид пульта сейчас. */
export function currentView(): View {
    return view;
}

/**
 * Применить выбор к странице: режим, тема и вид — в атрибуты <html>, токены темы — правилом в <style id="theme">, цвет
 * панели браузера — в meta theme-color. Помнит выбор человека, даже если его темы сейчас нет: плагин может прийти
 * позже, и тогда тема встанет сама.
 */
export function applyTheme(): void {
    const theme = currentTheme();
    const mode = modeOf(theme);
    const root = document.documentElement;
    root.dataset.theme = mode;
    root.dataset.themeId = theme.id;
    // Материал один — «Минимал»; вид — для правил стилей, которым надо знать раскладку.
    root.dataset.look = 'minimal';
    root.dataset.shell = view;
    let style = document.getElementById('theme') as HTMLStyleElement | null;
    if (!style) {
        style = document.createElement('style');
        style.id = 'theme';
        document.head.appendChild(style);
    }
    const css = activeCss(theme, mode);
    if (style.textContent !== css) style.textContent = css;
    const bg = theme.palette[mode].bg;
    if (bg) document.querySelector('meta[name="theme-color"]')?.setAttribute('content', bg);
    // Окно колонки у помощника (via.ts) — та же страница и то же хранилище, но темы пакетов у колонки свои: её пульт
    // красится выбором компьютера и ничего в его память не пишет, иначе пульт компьютера мигнёт чужой темой.
    if (VIA) return;
    store(CHOICE_KEY, JSON.stringify({ v: 2, id: choice.id, mode: choice.mode }));
    store(LEGACY_KEY, mode);
    store(VIEW_KEY, view);
    store(CSS_KEY, JSON.stringify({ id: theme.id, mode, css, look: 'minimal', layout: view }));
}

function changed(): void {
    applyTheme();
    for (const listener of [...listeners]) listener();
}

function choose(next: Choice): void {
    choice = next;
    changed();
}

/** Другая тема — в том же режиме, что на экране: смена темы не перекидывает из светлого в тёмный. */
export const setTheme = (id: string): void => choose({ id, mode: choice.mode ?? modeOf(currentTheme()) });
export const setMode = (mode: Mode): void => choose({ ...choice, mode });
export const setView = (next: View): void => {
    view = next;
    changed();
};

// Плагин принёс или убрал тему: выбранная могла появиться или пропасть.
onRegistryChange(() => {
    applyTheme();
    for (const listener of [...listeners]) listener();
});

/** Тема, режим и вид для компонентов: перерисовка, когда их меняют здесь или в другом месте пульта. */
export function useTheme(): {
    theme: Theme;
    mode: Mode;
    view: View;
    themes: Theme[];
    setTheme: (id: string) => void;
    setMode: (mode: Mode) => void;
    setView: (view: View) => void;
    toggleMode: () => void;
} {
    const [, rerender] = useState(0);
    useEffect(() => {
        const update = () => rerender((n) => n + 1);
        listeners.add(update);
        return () => {
            listeners.delete(update);
        };
    }, []);
    const theme = currentTheme();
    const mode = modeOf(theme);
    return {
        theme,
        mode,
        view,
        themes: availableThemes(),
        setTheme,
        setMode,
        setView,
        toggleMode: () => setMode(mode === 'dark' ? 'light' : 'dark'),
    };
}
