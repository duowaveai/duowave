// Аватар в верхней строке (docs/plans/panel-rebuild.md): кто вошёл, тема в одно нажатие — светлая, тёмная или «Авто», «Оформление»,
// «Люди и доступ» и выход. Луна в шапке переехала сюда; всё остальное об оформлении — «Настройки» → «Оформление».

import { useEffect, useRef, useState } from 'preact/hooks';

import { sessionStore } from '../api';
import { IconAuto, IconMoon, IconSun } from '../icons';
import { ModePick, useTheme } from './theme';
import { tr } from '../i18n';

/** Тема в одно нажатие; «Авто» — как у системы: светлая днём, тёмная вечером, и в темноте не режет глаза. */
const modes = (): Array<{ id: ModePick; label: string; Icon: typeof IconSun }> => [
    { id: 'light', label: tr.shell.modeLight, Icon: IconSun },
    { id: 'dark', label: tr.shell.modeDark, Icon: IconMoon },
    { id: 'auto', label: tr.shell.modeAuto, Icon: IconAuto },
];

/** Первая буква имени — на аватаре; без имени — «Я». */
const initial = (name: string | undefined): string => (name?.trim()[0] ?? tr.shell.initialFallback).toUpperCase();

export function AccountMenu({ onLook, onAccess, onSignOut }: { onLook: () => void; onAccess: () => void; onSignOut: () => void }) {
    const [open, setOpen] = useState(false);
    const box = useRef<HTMLDivElement>(null);
    const button = useRef<HTMLButtonElement>(null);
    const theme = useTheme();
    const me = sessionStore.get();
    // Нажатие мимо меню и Esc закрывают его; фокус — обратно на аватар.
    useEffect(() => {
        if (!open) return;
        const away = (event: PointerEvent) => {
            if (!box.current?.contains(event.target as Node)) setOpen(false);
        };
        const key = (event: KeyboardEvent) => {
            if (event.key !== 'Escape' || event.defaultPrevented) return;
            event.preventDefault();
            setOpen(false);
            button.current?.focus();
        };
        window.addEventListener('pointerdown', away, true);
        window.addEventListener('keydown', key, true);
        return () => {
            window.removeEventListener('pointerdown', away, true);
            window.removeEventListener('keydown', key, true);
        };
    }, [open]);
    const pick = (job: () => void) => {
        setOpen(false);
        job();
    };
    return (
        <div class="account" ref={box}>
            <button
                ref={button}
                class="account-btn"
                aria-label={tr.shell.accountButton(me?.name ?? tr.shell.signIn)}
                aria-haspopup="menu"
                aria-expanded={open}
                onClick={() => setOpen(!open)}
            >
                {initial(me?.name)}
            </button>
            {open && (
                <div class="account-menu menu" role="menu" aria-label={tr.shell.account}>
                    <div class="account-who">
                        <b>{me?.name ?? tr.shell.you}</b>
                        {me?.role && <span class="w-cap">{tr.shell.roleWords[me.role] ?? me.role}</span>}
                    </div>
                    <div class="account-theme" role="group" aria-label={tr.shell.themeLabel}>
                        {modes().map(({ id, label, Icon }) => (
                            <button key={id} role="menuitemradio" aria-checked={theme.pick === id} class={theme.pick === id ? 'on' : ''} onClick={() => theme.setMode(id)}>
                                <Icon size={16} />
                                {label}
                            </button>
                        ))}
                    </div>
                    <button role="menuitem" class="tab-more-item" onClick={() => pick(onLook)}>
                        {tr.shell.look}
                    </button>
                    <button role="menuitem" class="tab-more-item" onClick={() => pick(onAccess)}>
                        {tr.shell.peopleAccess}
                    </button>
                    <button role="menuitem" class="tab-more-item" onClick={() => pick(onSignOut)}>
                        {tr.shell.signOut}
                    </button>
                </div>
            )}
        </div>
    );
}
