// Боковая панель — второй вид пульта (docs/plans/desk.md): слева панель — роль, колонка, капсула «Сказать» (микрофон
// пульта, как в доке), разделы, кнопки шапки; справа — стол столбцом или открытый раздел страницей. Рамка по роли: у помощника
// панель тёмная. На узком экране панель уезжает: сверху полоса с меню, колонкой и кругом «Сказать», разделы выезжают
// слева по кнопке меню. Плашка «Делаю виджет» — в панели под колонкой, на узком экране — над содержимым. Полоса голоса —
// внизу содержимого: что услышано и что отвечают (docs/plans/mic-mode.md).

import { useEffect, useRef, useState } from 'preact/hooks';

import { IconLaptop, IconMenu, IconMic, IconMicOff } from '../icons';
import { micOn, onMic } from '../voice/mic';
import { useMicHeld } from '../voice/phrases';
import { VoiceStrip } from '../voice/strip';
import { DuoWaveLogo, DuoWaveMark, PoweredBy } from '../ui/duowave';
import { useDialogFocus } from '../ui/sheet';
import { AgentPlaque } from './agent-plaque';
import type { Nav, NavEntry, ShellProps } from './shell';
import { tr } from '../i18n';

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

function NavList({ nav, active, onNav }: { nav: Nav; active: string; onNav: (entry: NavEntry) => void }) {
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
        <nav class="side-nav" aria-label={tr.shell.sections}>
            {nav.left.map(item)}
            {nav.right.map(item)}
        </nav>
    );
}

/** Разделы на узком экране: панель выезжает слева поверх содержимого; Esc и нажатие мимо закрывают. */
function Drawer({ children, onClose }: { children: preact.ComponentChildren; onClose: () => void }) {
    const panel = useRef<HTMLElement>(null);
    useDialogFocus(panel, onClose);
    return (
        <div class="side-layer">
            <button class="side-scrim" aria-label={tr.shell.closeSections} tabIndex={-1} onClick={onClose} />
            <aside ref={panel} class="side-panel side-drawer glass-strong" role="dialog" aria-modal="true" aria-label={tr.shell.sections}>
                {children}
            </aside>
        </div>
    );
}

export function SidebarShell({ nav, active, onNav, top, station, actions, talk, notice, desk, window: win }: ShellProps) {
    const wide = useWide();
    const [open, setOpen] = useState(false);
    // Слушает ли пульт сейчас и не выключен ли микрофон полем ключа — как у кнопки дока.
    const [listening, setListening] = useState(micOn);
    useEffect(() => onMic(setListening), []);
    const held = useMicHeld();
    const talkLabel = held ? tr.shell.micHeld : listening ? tr.shell.endPhrase : tr.shell.say;
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
                    {tr.shell.thisComputer}
                </span>
            )}
            {win.content}
        </>
    ) : (
        desk
    );
    const role = <div class="side-role">{top.line}</div>;
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
                    <button class={`side-talk${listening ? ' is-listening' : ''}`} aria-pressed={listening} disabled={talk.disabled || held} onClick={talk.onTalk}>
                        {held ? <IconMicOff size={18} /> : <IconMic size={18} />}
                        {listening ? tr.shell.listening : tr.shell.say}
                    </button>
                    <NavList nav={nav} active={active} onNav={go} />
                    <div class="side-foot">{actions}</div>
                </aside>
                <main class="side-main">
                    {notice}
                    {page}
                    <PoweredBy />
                </main>
                <VoiceStrip />
            </div>
        );
    return (
        <div class="side-shell">
            <header class="side-top glass-strong">
                <button class="icon-btn glass" aria-label={tr.shell.sections} aria-expanded={open} onClick={() => setOpen(true)}>
                    <IconMenu />
                </button>
                <DuoWaveMark size={26} title="DuoWave" live />
                {station}
                <button
                    aria-label={talkLabel}
                    aria-pressed={listening}
                    class={`talk-btn side-talk-round${listening ? ' is-listening' : ''}`}
                    disabled={talk.disabled || held}
                    onClick={talk.onTalk}
                >
                    {held ? <IconMicOff size={20} /> : <IconMic size={20} />}
                </button>
            </header>
            <main class="side-main">
                <AgentPlaque place="side" />
                {notice}
                {page}
                <PoweredBy />
            </main>
            <VoiceStrip />
            {open && (
                <Drawer onClose={() => setOpen(false)}>
                    {role}
                    <NavList nav={nav} active={active} onNav={go} />
                    <div class="side-foot">{actions}</div>
                </Drawer>
            )}
        </div>
    );
}
