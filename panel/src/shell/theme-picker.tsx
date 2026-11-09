// «Вид пульта» (Настройки → Оформление): тема и её режим в этом браузере. Каждая тема — строкой со схемой своей раскладки в
// своих цветах: видно, что тема меняет не только цвета. Режим — сегментом с названиями режимов темы. Администратор
// может сделать свою тему на основе выбранной — черновиком в «Плагины» → «Мастерская».

import { can, sessionStore } from '../api';
import { IconCheck } from '../icons';
import { Action } from '../ui/cards';
import { usePanel } from './context';
import { useTheme } from './theme';
import type { Mode, Theme } from './themes';

/** Схема раскладки темы её же цветами: где навигация, где содержимое, где лаймовая кнопка «Сказать». */
function LayoutGlyph({ theme, mode }: { theme: Theme; mode: Mode }) {
    const tone = theme.palette[mode];
    const panel = { fill: tone['glass-bg'], stroke: tone['glass-border'] };
    const strong = { fill: tone['strong-bg'], stroke: tone['strong-border'] };
    return (
        <svg class="theme-glyph" width="52" height="40" viewBox="0 0 52 40" aria-hidden="true">
            <rect x="0.5" y="0.5" width="51" height="39" rx="10" fill={tone.bg} stroke={tone['glass-border']} />
            {theme.layout === 'sidebar' ? (
                <>
                    <rect x="4" y="4" width="15" height="32" rx="6" {...strong} />
                    <rect x="7" y="8.5" width="9" height="3.5" rx="1.75" fill={tone.accent} />
                    <rect x="22" y="5" width="26" height="10" rx="4" {...panel} />
                    <rect x="22" y="18" width="26" height="17" rx="4" {...panel} />
                </>
            ) : (
                <>
                    <rect x="6" y="5" width="40" height="10" rx="4" {...panel} />
                    <rect x="6" y="17" width="40" height="10" rx="4" {...panel} />
                    <rect x="8" y="29.5" width="36" height="7" rx="3.5" {...strong} />
                    <circle cx="26" cy="33" r="2.8" fill={tone.accent} />
                </>
            )}
        </svg>
    );
}

const isAdmin = (): boolean => can('plugins.manage') && sessionStore.get()?.role === 'admin';

export function ThemeCard() {
    const panel = usePanel();
    const { theme, mode, themes, setTheme, setMode } = useTheme();
    return (
        <section class="card glass card-list" aria-labelledby="theme-h">
            <div class="section-head">
                <h2 id="theme-h" class="t-title-xs">
                    Вид пульта
                </h2>
                <span class="t-meta">в этом браузере</span>
            </div>
            <div role="radiogroup" aria-label="Тема пульта" class="flex flex-col">
                {themes.map((entry) => {
                    const on = entry.id === theme.id;
                    return (
                        <button key={entry.id} role="radio" aria-checked={on} class="dash-option" onClick={() => setTheme(entry.id)}>
                            <LayoutGlyph theme={entry} mode={mode} />
                            <span class="dash-option-text">
                                <b>{entry.title}</b>
                                <span class="t-caption">{entry.about}</span>
                            </span>
                            <span class={`dash-radio${on ? ' on' : ''}`} aria-hidden="true">
                                {on && <IconCheck />}
                            </span>
                        </button>
                    );
                })}
            </div>
            <div role="radiogroup" aria-label="Режим темы" class="look-seg glass theme-modes">
                {(['dark', 'light'] as const).map((value) => (
                    <button key={value} role="radio" aria-checked={mode === value} onClick={() => setMode(value)}>
                        {theme.modes[value]}
                    </button>
                ))}
            </div>
            {theme.modeNotes?.[mode] && <p class="t-caption m-0">{theme.modeNotes[mode]}</p>}
            {isAdmin() && (
                <div class="flex flex-wrap items-center gap-8 mt-8">
                    <Action label="Сделать свою тему" onClick={() => panel.go('plugins', 'workshop', undefined, { new: 'theme', base: theme.id })} />
                    <span class="t-caption">Черновик в мастерской на основе «{theme.title}»: поменяйте цвета, примерьте, поставьте.</span>
                </div>
            )}
        </section>
    );
}
