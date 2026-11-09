// Боковая панель — раскладка для широкого экрана (DESIGN.md, «Компьютер»): слева плавающая панель плотного стекла —
// колонка, капсула «Сказать через колонку», разделы, кнопки шапки; справа — содержимое. На узком экране панель
// уезжает: сверху остаётся полоса с меню, колонкой и кругом «Сказать», а разделы выезжают слева по кнопке меню.

import { useEffect, useRef, useState } from 'preact/hooks';

import { IconMenu, IconMic } from '../icons';
import { DuoWaveLogo, DuoWaveMark, PoweredBy } from '../ui/duowave';
import { useDialogFocus } from '../ui/sheet';
import type { SectionDef } from './sections';
import { ShellProps, splitNav } from './shell';

/** С этой ширины панель стоит сбоку постоянно. */
const WIDE = '(min-width: 900px)';

function useWide(): boolean {
    const [wide, setWide] = useState(() => matchMedia(WIDE).matches);
    useEffect(() => {
        const query = matchMedia(WIDE);
        const update = () => setWide(query.matches);
        update();
        query.addEventListener('change', update);
        return () => query.removeEventListener('change', update);
    }, []);
    return wide;
}

function NavList({ sections, active, onGo }: { sections: SectionDef[]; active: string; onGo: (id: string) => void }) {
    const { main, more } = splitNav(sections);
    const item = (section: SectionDef) => {
        const { icon: Icon, Badge } = section;
        return (
            <button key={section.id} class="side-item" aria-current={active === section.id ? 'page' : undefined} onClick={() => onGo(section.id)}>
                <span class="tab-icon">
                    <Icon size={20} />
                    {Badge && <Badge />}
                </span>
                <span class="side-item-label">{section.title}</span>
            </button>
        );
    };
    return (
        <nav class="side-nav" aria-label="Разделы">
            {main.map(item)}
            {more.length > 0 && <span class="side-sep" aria-hidden="true" />}
            {more.map(item)}
        </nav>
    );
}

/** Разделы на узком экране: панель выезжает слева поверх содержимого; Esc и нажатие мимо закрывают. */
function Drawer({ children, onClose }: { children: preact.ComponentChildren; onClose: () => void }) {
    const panel = useRef<HTMLElement>(null);
    useDialogFocus(panel, onClose);
    return (
        <div class="side-layer">
            <button class="side-scrim" aria-label="Закрыть разделы" tabIndex={-1} onClick={onClose} />
            <aside ref={panel} class="side-panel side-drawer glass-strong" role="dialog" aria-modal="true" aria-label="Разделы">
                {children}
            </aside>
        </div>
    );
}

export function SidebarShell({ sections, active, onGo, station, actions, talk, notice, children }: ShellProps) {
    const wide = useWide();
    const [open, setOpen] = useState(false);
    // Выбрали раздел — панель на телефоне больше не нужна.
    const go = (id: string) => {
        setOpen(false);
        onGo(id);
    };
    if (wide)
        return (
            <div class="side-shell">
                <aside class="side-panel glass-strong">
                    <div class="side-brand">
                        <DuoWaveLogo size={26} live />
                    </div>
                    <div class="side-station">{station}</div>
                    <button class="side-talk" disabled={talk.disabled} onClick={talk.onTalk}>
                        <IconMic size={18} />
                        Сказать через колонку
                    </button>
                    <NavList sections={sections} active={active} onGo={go} />
                    <div class="side-foot">{actions}</div>
                </aside>
                <main class="side-main">
                    {notice}
                    {children}
                    <PoweredBy />
                </main>
            </div>
        );
    return (
        <div class="side-shell">
            <header class="side-top glass-strong">
                <button class="icon-btn glass" aria-label="Разделы" aria-expanded={open} onClick={() => setOpen(true)}>
                    <IconMenu />
                </button>
                <DuoWaveMark size={26} title="DuoWave" live />
                {station}
                <button aria-label="Сказать через колонку" class="talk-btn side-talk-round" disabled={talk.disabled} onClick={talk.onTalk}>
                    <IconMic size={20} />
                </button>
            </header>
            <main class="side-main">
                {notice}
                {children}
                <PoweredBy />
            </main>
            {open && (
                <Drawer onClose={() => setOpen(false)}>
                    <NavList sections={sections} active={active} onGo={go} />
                    <div class="side-foot">{actions}</div>
                </Drawer>
            )}
        </div>
    );
}
