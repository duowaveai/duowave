// Выбор темы и режима: что выбрано, применение к странице, память в этом браузере. Сами темы — данные (themes.ts),
// приходят через реестр (definePlugin). Первую отрисовку до скриптов делает index.html: он читает тот же выбор.

import { useEffect, useState } from 'preact/hooks';

import { onRegistryChange, registeredPlugins } from './registry';
import { DEFAULT_THEME, Mode, Theme, activeCss } from './themes';

/** Выбор человека: { v: 2, id, mode } — null там, где он не выбирал и действует тема или режим по умолчанию. */
const CHOICE_KEY = 'panel-theme';
/** Прежний ключ пульта — только режим «Стекла». Пишется для старых сборок. */
const LEGACY_KEY = 'glass-theme';
/** CSS последней применённой темы: index.html вставляет его до загрузки, и выбранная тема не мигает «Стеклом». */
const CSS_KEY = 'panel-theme-css';

interface Choice {
    /** Тема, которую выбрал человек; null — тема по умолчанию. */
    id: string | null;
    /** Режим, который выбрал человек; null — режим темы, а у темы без своего — как у системы. */
    mode: Mode | null;
}

/**
 * Выбор, с которым страница уже отрисована: index.html прочёл его до загрузки пульта (data-theme-pick и
 * data-mode-pick на <html>). Прежние сборки записывали «Стекло» сами, без выбора человека, — такой выбор index.html
 * не считает выбором: тема по умолчанию стала другой, и она встаёт в своём режиме.
 */
function initialChoice(): Choice {
    const root = document.documentElement;
    const mode = root.dataset.modePick;
    return { id: root.dataset.themePick || null, mode: mode === 'light' || mode === 'dark' ? mode : null };
}

let choice: Choice = initialChoice();
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

function store(key: string, value: string): void {
    try {
        localStorage.setItem(key, value);
    } catch {
        // приватное окно: выбор живёт до перезагрузки
    }
}

/** Темы всех включённых плагинов; встроенные — первыми. Тема с тем же id из плагина позже заменяет прежнюю. */
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

/**
 * Применить выбор к странице: режим и тема — в атрибуты <html>, токены темы — правилом в <style id="theme">, цвет
 * панели браузера — в meta theme-color. Помнит выбор человека, даже если его темы сейчас нет: плагин может прийти
 * позже, и тогда тема встанет сама.
 */
export function applyTheme(): void {
    const theme = currentTheme();
    const mode = modeOf(theme);
    const root = document.documentElement;
    root.dataset.theme = mode;
    root.dataset.themeId = theme.id;
    // Материал и раскладка — для правил стилей, которым их надо знать (ui/minimal.css; где выпадает окно колонок).
    root.dataset.look = theme.look ?? 'glass';
    root.dataset.shell = theme.layout;
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
    store(CHOICE_KEY, JSON.stringify({ v: 2, id: choice.id, mode: choice.mode }));
    store(LEGACY_KEY, mode);
    store(CSS_KEY, JSON.stringify({ id: theme.id, mode, css, look: theme.look ?? 'glass', layout: theme.layout }));
}

function choose(next: Choice): void {
    choice = next;
    applyTheme();
    for (const listener of [...listeners]) listener();
}

/** Другая тема — в том же режиме, что на экране: смена темы не перекидывает из светлого в тёмный. */
export const setTheme = (id: string): void => choose({ id, mode: choice.mode ?? modeOf(currentTheme()) });
export const setMode = (mode: Mode): void => choose({ ...choice, mode });

// Плагин принёс или убрал тему: выбранная могла появиться или пропасть.
onRegistryChange(() => {
    applyTheme();
    for (const listener of [...listeners]) listener();
});

/** Тема и режим для компонентов: перерисовка, когда их меняют здесь или в другом месте пульта. */
export function useTheme(): { theme: Theme; mode: Mode; themes: Theme[]; setTheme: (id: string) => void; setMode: (mode: Mode) => void; toggleMode: () => void } {
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
        themes: availableThemes(),
        setTheme,
        setMode,
        toggleMode: () => setMode(mode === 'dark' ? 'light' : 'dark'),
    };
}
