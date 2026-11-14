// «Вид пульта» (Настройки → Оформление): вид, режим и тема в этом браузере (docs/plans/desk.md). Вид — стол и док или
// боковая панель — схемой раскладки; режим — сегментом; тема — встроенный «Минимал» и темы пакетов, которые его
// перекрашивают. Администратор может сделать свою тему на основе выбранной — черновиком в «Плагины» → «Мастерская».

import { can, sessionStore } from '../api';
import { IconCheck } from '../icons';
import { Action } from '../ui/cards';
import { usePanel } from './context';
import { View, useTheme } from './theme';
import { MINIMAL } from './themes';
import type { Mode, Theme } from './themes';
import { byMode } from '../words';
import { tr } from '../i18n';

/** Схема вида цветами темы: стол с колонкой и доком снизу или боковая панель с содержимым справа. */
function ViewGlyph({ theme, mode, view }: { theme: Theme; mode: Mode; view: View }) {
    const tone = theme.palette[mode];
    const card = { fill: tone['glass-bg'], stroke: tone['glass-border'] };
    const sheet = { fill: tone['strong-bg'], stroke: tone['strong-border'] };
    return (
        <svg class="theme-glyph" width="52" height="40" viewBox="0 0 52 40" aria-hidden="true">
            <rect x="0.5" y="0.5" width="51" height="39" rx="10" fill={tone.bg} stroke={tone['glass-border']} />
            {view === 'sidebar' ? (
                <>
                    <rect x="4" y="4" width="15" height="32" rx="6" {...sheet} />
                    <rect x="7" y="8.5" width="9" height="3.5" rx="1.75" fill={tone.accent} />
                    <rect x="22" y="5" width="26" height="10" rx="4" {...card} />
                    <rect x="22" y="18" width="26" height="17" rx="4" {...card} />
                </>
            ) : (
                <>
                    <rect x="4" y="6" width="11" height="8" rx="2.5" {...card} />
                    <rect x="4" y="16" width="11" height="8" rx="2.5" {...card} />
                    <rect x="37" y="6" width="11" height="8" rx="2.5" {...card} />
                    <circle cx="26" cy="15" r="6.5" fill="none" stroke={tone.orbit ?? tone['glass-border']} />
                    <circle cx="26" cy="15" r="2.6" fill={tone['bar-off']} />
                    <rect x="12" y="29.5" width="28" height="7" rx="3.5" {...sheet} />
                    <circle cx="26" cy="33" r="2.4" fill={tone.accent} />
                </>
            )}
        </svg>
    );
}

// Читается при отрисовке: язык меняется, а в режиме «Микрофон» посередине не колонка.
const views = (): Array<{ id: View; title: string; about: string }> => [
    { id: 'desk', title: tr.shell.viewDeskTitle, about: byMode(tr.shell.viewDeskAbout, tr.shell.viewDeskAboutMic) },
    { id: 'sidebar', title: tr.shell.viewSidebarTitle, about: tr.shell.viewSidebarAbout },
];

// Встроенная тема лежит в themes.ts по-русски (файл читает и vite.config.ts): на язык пульта её подписи переводятся здесь,
// по совпадению с исходными; темы пакетов и производные от неё со своими подписями остаются как есть.
const themeTitle = (theme: Theme): string => (theme.title === MINIMAL.title ? tr.shell.themeMinimalTitle : theme.title);
const themeAbout = (theme: Theme): string => (theme.about === MINIMAL.about ? tr.shell.themeMinimalAbout : theme.about);
const modeName = (theme: Theme, mode: Mode): string =>
    theme.modes[mode] === MINIMAL.modes[mode] ? (mode === 'dark' ? tr.shell.themeMinimalDark : tr.shell.themeMinimalLight) : theme.modes[mode];

const isAdmin = (): boolean => can('plugins.manage') && sessionStore.get()?.role === 'admin';

function Radio({ on, onPick, glyph, title, about }: { on: boolean; onPick: () => void; glyph: preact.ComponentChildren; title: string; about: string }) {
    return (
        <button role="radio" aria-checked={on} class="dash-option" onClick={onPick}>
            {glyph}
            <span class="dash-option-text">
                <b>{title}</b>
                <span class="t-caption">{about}</span>
            </span>
            <span class={`dash-radio${on ? ' on' : ''}`} aria-hidden="true">
                {on && <IconCheck />}
            </span>
        </button>
    );
}

export function ThemeCard() {
    const panel = usePanel();
    const { theme, mode, pick, view, themes, setTheme, setMode, setView } = useTheme();
    return (
        <section class="card glass card-list" aria-labelledby="theme-h">
            <div class="section-head">
                <h2 id="theme-h" class="t-title-xs">
                    {tr.shell.viewTitle}
                </h2>
                <span class="t-meta">{tr.shell.inThisBrowser}</span>
            </div>
            <div role="radiogroup" aria-label={tr.shell.viewTitle} class="flex flex-col">
                {views().map((entry) => (
                    <Radio
                        key={entry.id}
                        on={view === entry.id}
                        onPick={() => setView(entry.id)}
                        glyph={<ViewGlyph theme={theme} mode={mode} view={entry.id} />}
                        title={entry.title}
                        about={entry.about}
                    />
                ))}
            </div>
            <div role="radiogroup" aria-label={tr.shell.modeLabel} class="look-seg glass theme-modes">
                {(['light', 'dark', 'auto'] as const).map((value) => (
                    <button key={value} role="radio" aria-checked={pick === value} onClick={() => setMode(value)}>
                        {value === 'auto' ? tr.shell.modeAuto : modeName(theme, value)}
                    </button>
                ))}
            </div>
            {pick === 'auto' && <p class="t-caption m-0">{tr.shell.autoNote(modeName(theme, mode).toLowerCase())}</p>}
            {theme.modeNotes?.[mode] && <p class="t-caption m-0">{theme.modeNotes[mode]}</p>}
            {themes.length > 1 && (
                <>
                    <h3 class="t-meta m-0 mt-8">{tr.shell.themeLabel}</h3>
                    <div role="radiogroup" aria-label={tr.shell.themePanelLabel} class="flex flex-col">
                        {themes.map((entry) => (
                            <Radio
                                key={entry.id}
                                on={entry.id === theme.id}
                                onPick={() => setTheme(entry.id)}
                                glyph={<ViewGlyph theme={entry} mode={mode} view={view} />}
                                title={themeTitle(entry)}
                                about={themeAbout(entry)}
                            />
                        ))}
                    </div>
                </>
            )}
            {isAdmin() && (
                <div class="flex flex-wrap items-center gap-8 mt-8">
                    <Action label={tr.shell.makeOwnTheme} onClick={() => panel.go('skills', 'workshop', undefined, { new: 'theme', base: theme.id })} />
                    <span class="t-caption">{tr.shell.makeOwnThemeAbout(themeTitle(theme))}</span>
                </div>
            )}
        </section>
    );
}
