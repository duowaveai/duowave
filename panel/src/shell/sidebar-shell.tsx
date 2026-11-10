// Боковая панель — второй вид пульта (docs/plans/desk.md): слева панель — роль, колонка, капсула «Сказать через
// колонку», разделы, кнопки шапки; справа — стол столбцом или открытый раздел страницей. Рамка по роли: у помощника
// панель тёмная. На узком экране панель уезжает: сверху полоса с меню, колонкой и кругом «Сказать», разделы выезжают
// слева по кнопке меню. Плашка «Делаю виджет» — в панели под колонкой, на узком экране — над содержимым.

import { useEffect, useRef, useState } from 'preact/hooks';

import { IconLaptop, IconMenu, IconMic, IconPlus } from '../icons';
import { DuoWaveLogo, DuoWaveMark, PoweredBy } from '../ui/duowave';
import { useDialogFocus } from '../ui/sheet';
import { AgentPlaque } from './agent-plaque';
import type { Nav, NavEntry, ShellProps } from './shell';

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

function NavList({ nav, active, onNav, helper }: { nav: Nav; active: string; onNav: (entry: NavEntry) => void; helper: boolean }) {
    const item = (entry: NavEntry) => {
        const { icon: Icon, Badge } = entry;
        return (
            <button key={entry.key} class="side-item" aria-current={active === entry.key ? 'page' : undefined} onClick={() => onNav(entry)}>
                <span class="tab-icon">
                    <Icon size={20} />
                    {Badge && <Badge />}
                </span>
                <span class="side-item-label">{entry.title}</span>
            </button>
        );
    };
    return (
        <nav class="side-nav" aria-label="Разделы">
            {helper && nav.left.length > 0 && <span class="side-group">Этот компьютер</span>}
            {nav.left.map(item)}
            {helper && nav.right.length > 0 && <span class="side-group">Колонка</span>}
            {nav.right.map(item)}
            {nav.more.length > 0 && <span class="side-sep" aria-hidden="true" />}
            {nav.more.map(item)}
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

export function SidebarShell({ nav, active, onNav, top, station, actions, talk, onAdd, notice, desk, window: win }: ShellProps) {
    const wide = useWide();
    const [open, setOpen] = useState(false);
    const helper = top.role === 'helper';
    // Выбрали раздел — панель на телефоне больше не нужна.
    const go = (entry: NavEntry) => {
        setOpen(false);
        onNav(entry);
    };
    const page = win ? (
        <>
            {win.scope === 'own' && (
                <span class="role-label self-start">
                    <IconLaptop size={13} />
                    Этот компьютер
                </span>
            )}
            {win.content}
        </>
    ) : (
        <>
            {desk}
            <button class="side-add" onClick={onAdd}>
                <IconPlus size={18} />
                Новый виджет
            </button>
        </>
    );
    const role = (
        <div class="side-role">
            <span class="side-role-title">{top.title}</span>
            <span class="side-role-sub">{top.subtitle}</span>
            {top.chips && <span class="side-role-chips">{top.chips}</span>}
        </div>
    );
    if (wide)
        return (
            <div class="side-shell">
                <aside class="side-panel glass-strong">
                    <div class="side-brand">
                        <DuoWaveLogo size={26} live />
                    </div>
                    {role}
                    <div class="side-station">{station}</div>
                    <AgentPlaque place="side" />
                    <button class="side-talk" disabled={talk.disabled} onClick={talk.onTalk}>
                        <IconMic size={18} />
                        Сказать через колонку
                    </button>
                    <NavList nav={nav} active={active} onNav={go} helper={helper} />
                    <div class="side-foot">{actions}</div>
                </aside>
                <main class="side-main">
                    {notice}
                    {page}
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
                <AgentPlaque place="side" />
                {notice}
                {page}
                <PoweredBy />
            </main>
            {open && (
                <Drawer onClose={() => setOpen(false)}>
                    {role}
                    <NavList nav={nav} active={active} onNav={go} helper={helper} />
                    <div class="side-foot">{actions}</div>
                </Drawer>
            )}
        </div>
    );
}
