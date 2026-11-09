// Панель вкладок снизу — раскладка «Стекла» для телефона (DESIGN.md, Navigation): шапка с колонкой, содержимое
// одной колонкой, плавающая панель плотного стекла; круг «Сказать» посередине, разделы по бокам от него.

import { useState } from 'preact/hooks';
import { IconMenu, IconMic } from '../icons';
import { DuoWaveMark, PoweredBy } from '../ui/duowave';
import type { SectionDef } from './sections';
import { ShellProps, splitNav } from './shell';

function Tab({ section, active, onGo }: { section: SectionDef; active: boolean; onGo: (id: string) => void }) {
    const { icon: Icon, Badge } = section;
    return (
        <button class="tab" aria-current={active ? 'page' : undefined} onClick={() => onGo(section.id)}>
            {Badge ? (
                <span class="tab-icon">
                    <Icon size={22} />
                    <Badge />
                </span>
            ) : (
                <Icon size={22} />
            )}
            {section.title}
        </button>
    );
}

export function TabbarShell({ sections, active, onGo, station, actions, talk, notice, children }: ShellProps) {
    // Журнал, мастерская и прочие «ещё» в панель не помещаются: они в меню «Ещё» — последней вкладкой.
    const { main, more } = splitNav(sections);
    const [open, setOpen] = useState(false);
    const go = (id: string) => {
        setOpen(false);
        onGo(id);
    };
    // «Ещё» — такая же вкладка, как разделы: круг «Сказать» стоит посередине всех, поровну по бокам (2 + 2 при трёх
    // разделах и «Ещё»).
    const tabs = more.length > 0 ? main.length + 1 : main.length;
    const half = Math.ceil(tabs / 2);
    return (
        <div class="screen">
            <header class="top-head">
                <DuoWaveMark size={28} title="DuoWave" live />
                {station}
                {actions}
            </header>

            {notice}

            {children}

            <PoweredBy />

            {open && (
                <>
                    <div class="tab-more-scrim" onClick={() => setOpen(false)} />
                    <div class="tab-more glass-strong" role="menu" aria-label="Ещё разделы">
                        {more.map(({ id, title, icon: Icon }) => (
                            <button
                                key={id}
                                role="menuitem"
                                class="tab-more-item"
                                aria-current={active === id ? 'page' : undefined}
                                onClick={() => go(id)}
                            >
                                <Icon size={20} />
                                {title}
                            </button>
                        ))}
                    </div>
                </>
            )}

            <nav class="tabbar glass-strong" aria-label="Разделы" data-dock="bottom" style={{ '--cells': tabs + 1 }}>
                {main.slice(0, half).map((section) => (
                    <Tab key={section.id} section={section} active={active === section.id} onGo={onGo} />
                ))}
                <button aria-label="Сказать через колонку" class="talk-btn" disabled={talk.disabled} onClick={talk.onTalk}>
                    <IconMic />
                </button>
                {main.slice(half).map((section) => (
                    <Tab key={section.id} section={section} active={active === section.id} onGo={onGo} />
                ))}
                {more.length > 0 && (
                    <button
                        class="tab"
                        aria-haspopup="menu"
                        aria-expanded={open}
                        aria-current={more.some((section) => section.id === active) ? 'page' : undefined}
                        onClick={() => setOpen(!open)}
                    >
                        <IconMenu size={22} />
                        Ещё
                    </button>
                )}
            </nav>
        </div>
    );
}
