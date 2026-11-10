// «Стол и док» — основной вид пульта (docs/plans/desk.md, макеты docs/design/roles): тонкая верхняя строка с ролью,
// стол во весь экран, внизу док — разделы, круг «Сказать», «Ещё» — и отдельная «+» для нового виджета. Разделы
// открываются окнами поверх стола; верхняя строка и док остаются видны. Рамка (строка и док) — по роли: у шлюза белая,
// у помощника тёмная (`<html data-role>`, ui/desk.css). Пока агент мастерской делает виджет — в строке его плашка.

import { useEffect, useRef, useState } from 'preact/hooks';

import { IconMenu, IconMic, IconPlus } from '../icons';
import { DuoWaveMark } from '../ui/duowave';
import { AgentPlaque } from './agent-plaque';
import type { NavEntry, ShellProps } from './shell';
import { DeskWindow } from './window';

/** Часы верхней строки: «07:42», раз в полминуты. */
export function useClock(): string {
    const format = () => new Date().toLocaleTimeString('ru-RU', { hour: '2-digit', minute: '2-digit' });
    const [time, setTime] = useState(format);
    useEffect(() => {
        const id = window.setInterval(() => setTime(format()), 30_000);
        return () => window.clearInterval(id);
    }, []);
    return time;
}

function Cell({ entry, active, onNav }: { entry: NavEntry; active: boolean; onNav: (entry: NavEntry) => void }) {
    const { icon: Icon, Badge } = entry;
    return (
        <button class="dock-cell" aria-current={active ? 'page' : undefined} onClick={() => onNav(entry)}>
            <span class="tab-icon">
                <Icon size={22} />
                {Badge && <Badge />}
            </span>
            <span class="dock-cell-label">{entry.title}</span>
            {active && <span class="dock-pip" aria-hidden="true" />}
        </button>
    );
}

export function DeskShell({ nav, active, onNav, top, actions, talk, onAdd, notice, desk, window: win }: ShellProps) {
    const [more, setMore] = useState(false);
    const moreButton = useRef<HTMLButtonElement>(null);
    const dock = useRef<HTMLElement>(null);
    const clock = useClock();
    // Док шире экрана (телефон, у помощника семь ячеек) прокручивается: открытый пункт — в видимой части.
    useEffect(() => {
        dock.current?.querySelector<HTMLElement>('[aria-current="page"]')?.scrollIntoView({ block: 'nearest', inline: 'nearest' });
    }, [active]);
    const go = (entry: NavEntry) => {
        setMore(false);
        onNav(entry);
    };
    // «Ещё» открыто: Esc закрывает меню — и только его, окно под ним остаётся; фокус — обратно на «Ещё».
    useEffect(() => {
        if (!more) return;
        const onKey = (event: KeyboardEvent) => {
            if (event.key !== 'Escape' || event.defaultPrevented) return;
            event.preventDefault();
            setMore(false);
            moreButton.current?.focus();
        };
        window.addEventListener('keydown', onKey, true);
        return () => window.removeEventListener('keydown', onKey, true);
    }, [more]);
    const moreActive = nav.more.some((entry) => entry.key === active);
    // Окно колонки у помощника несёт её полосу; своё — тёмную метку «Этот компьютер» (её ставит само окно).
    return (
        <div class="desk-shell">
            <header class="desk-top">
                <DuoWaveMark size={22} title="DuoWave" live />
                <span class="desk-top-role">{top.title}</span>
                <span class="desk-top-sub">{top.subtitle}</span>
                {top.narrow && <span class="desk-top-narrow">{top.narrow}</span>}
                <div class="desk-top-end">
                    <AgentPlaque place="top" />
                    {top.chips}
                    {actions}
                    <span class="desk-top-clock" aria-label={`Время: ${clock}`}>
                        {clock}
                    </span>
                </div>
            </header>

            <main class="desk-main" aria-hidden={win ? 'true' : undefined} inert={win ? true : undefined}>
                {notice}
                {desk}
            </main>

            {win && <DeskWindow key={win.id} win={win} />}

            {more && (
                <>
                    <div class="dock-more-scrim" onClick={() => setMore(false)} />
                    <div class="dock-more menu" role="menu" aria-label="Ещё разделы">
                        {nav.more.map((entry) => {
                            const { icon: Icon } = entry;
                            return (
                                <button
                                    key={entry.key}
                                    role="menuitem"
                                    class="tab-more-item"
                                    aria-current={active === entry.key ? 'page' : undefined}
                                    onClick={() => go(entry)}
                                >
                                    <Icon size={20} />
                                    {entry.title}
                                </button>
                            );
                        })}
                    </div>
                </>
            )}

            <div class="dock-wrap">
                <nav ref={dock} class="dock" aria-label={top.role === 'helper' ? 'Док помощника' : 'Док'} data-dock="bottom">
                    {nav.left.map((entry) => (
                        <Cell key={entry.key} entry={entry} active={active === entry.key} onNav={go} />
                    ))}
                    <button aria-label="Сказать через колонку" class="talk-btn dock-talk" disabled={talk.disabled} onClick={talk.onTalk}>
                        <IconMic size={22} />
                    </button>
                    {nav.right.map((entry) => (
                        <Cell key={entry.key} entry={entry} active={active === entry.key} onNav={go} />
                    ))}
                    {nav.more.length > 0 && (
                        <>
                            {top.role === 'helper' && <span class="dock-sep" aria-hidden="true" />}
                            <button
                                ref={moreButton}
                                class="dock-cell"
                                aria-haspopup="menu"
                                aria-expanded={more}
                                aria-current={moreActive ? 'page' : undefined}
                                onClick={() => setMore(!more)}
                            >
                                <IconMenu size={22} />
                                <span class="dock-cell-label">Ещё</span>
                                {moreActive && <span class="dock-pip" aria-hidden="true" />}
                            </button>
                        </>
                    )}
                </nav>
                <button class="dock-add" aria-label="Добавить виджет" onClick={onAdd}>
                    <IconPlus size={22} />
                </button>
            </div>
        </div>
    );
}
