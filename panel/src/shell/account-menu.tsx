// Аватар в верхней строке (docs/plans/panel-rebuild.md): кто вошёл, тема в одно нажатие — светлая, тёмная или «Авто», «Оформление»,
// «Люди и доступ» и выход. Луна в шапке переехала сюда; всё остальное об оформлении — «Настройки» → «Оформление».

import { useEffect, useRef, useState } from 'preact/hooks';

import { sessionStore } from '../api';
import { IconAuto, IconMoon, IconSun } from '../icons';
import { ModePick, useTheme } from './theme';

/** Тема в одно нажатие; «Авто» — как у системы: светлая днём, тёмная вечером, и в темноте не режет глаза. */
const MODES: Array<{ id: ModePick; label: string; Icon: typeof IconSun }> = [
    { id: 'light', label: 'Светлая', Icon: IconSun },
    { id: 'dark', label: 'Тёмная', Icon: IconMoon },
    { id: 'auto', label: 'Авто', Icon: IconAuto },
];

const ROLE_WORDS: Record<string, string> = { admin: 'администратор', member: 'домашний', guest: 'гость' };

/** Первая буква имени — на аватаре; без имени — «Я». */
const initial = (name: string | undefined): string => (name?.trim()[0] ?? 'Я').toUpperCase();

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
                aria-label={`${me?.name ?? 'Вход'}: тема, оформление, выход`}
                aria-haspopup="menu"
                aria-expanded={open}
                onClick={() => setOpen(!open)}
            >
                {initial(me?.name)}
            </button>
            {open && (
                <div class="account-menu menu" role="menu" aria-label="Аккаунт">
                    <div class="account-who">
                        <b>{me?.name ?? 'Вы'}</b>
                        {me?.role && <span class="w-cap">{ROLE_WORDS[me.role] ?? me.role}</span>}
                    </div>
                    <div class="account-theme" role="group" aria-label="Тема">
                        {MODES.map(({ id, label, Icon }) => (
                            <button key={id} role="menuitemradio" aria-checked={theme.pick === id} class={theme.pick === id ? 'on' : ''} onClick={() => theme.setMode(id)}>
                                <Icon size={16} />
                                {label}
                            </button>
                        ))}
                    </div>
                    <button role="menuitem" class="tab-more-item" onClick={() => pick(onLook)}>
                        Оформление
                    </button>
                    <button role="menuitem" class="tab-more-item" onClick={() => pick(onAccess)}>
                        Люди и доступ
                    </button>
                    <button role="menuitem" class="tab-more-item" onClick={() => pick(onSignOut)}>
                        Выйти
                    </button>
                </div>
            )}
        </div>
    );
}
