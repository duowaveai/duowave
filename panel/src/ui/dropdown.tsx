import { useEffect, useRef, useState } from 'preact/hooks';

/** Один вариант списка: имя и мелкая подсказка справа (например, количество голосов). */
export interface DropdownOption {
    id: string;
    label: string;
    hint?: string;
}

const CHECK = (
    <svg aria-hidden="true" width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round">
        <path d="M20 6 9 17l-5-5" />
    </svg>
);

/**
 * Выпадающий список фильтра, как Select в Хэнди: тихая кнопка с шевроном, под ней стеклянное окно с
 * вариантами — отмеченный галочкой, подсказка справа, «все» первым пунктом. Закрывается выбором, мимо
 * окна или Esc.
 */
export function Dropdown({
    label,
    anyLabel,
    value,
    options,
    onPick,
}: {
    /** Что написано на кнопке, пока ничего не выбрано. */
    label: string;
    /** Первый пункт окна: сбросить фильтр. */
    anyLabel: string;
    /** Выбранный вариант; null — фильтр не активен. */
    value: string | null;
    options: DropdownOption[];
    onPick: (id: string | null) => void;
}) {
    const [open, setOpen] = useState(false);
    const root = useRef<HTMLDivElement | null>(null);

    useEffect(() => {
        if (!open) return;
        const away = (event: MouseEvent): void => {
            if (!root.current?.contains(event.target as Node)) setOpen(false);
        };
        const escape = (event: KeyboardEvent): void => {
            if (event.key === 'Escape') setOpen(false);
        };
        document.addEventListener('mousedown', away);
        document.addEventListener('keydown', escape);
        return () => {
            document.removeEventListener('mousedown', away);
            document.removeEventListener('keydown', escape);
        };
    }, [open]);

    const chosen = options.find((option) => option.id === value);
    const row = (selected: boolean, text: string, hint: string | undefined, pick: () => void) => (
        <button
            key={text}
            role="option"
            aria-selected={selected}
            onClick={pick}
            class="dd-option"
        >
            <span class="flex-1 overflow-hidden text-ellipsis nowrap">{text}</span>
            {hint && <span class="t-caption flex-none">{hint}</span>}
            {selected && <span class="flex-none inline-flex">{CHECK}</span>}
        </button>
    );

    return (
        <div ref={root} class="relative flex-none">
            <button
                aria-haspopup="listbox"
                aria-expanded={open}
                onClick={() => setOpen(!open)}
                class={`dd-trigger${open || chosen ? ' on' : ''}`}
            >
                {!chosen && <span class="c-sub fw-600">{label}</span>}
                {chosen?.label ?? anyLabel}
                <svg
                    aria-hidden="true"
                    width="14"
                    height="14"
                    viewBox="0 0 24 24"
                    fill="none"
                    stroke="currentColor"
                    stroke-width="2.5"
                    stroke-linecap="round"
                    stroke-linejoin="round"
                    class="dd-chevron"
                >
                    <path d="M6 9l6 6 6-6" />
                </svg>
            </button>
            {open && (
                <div
                    role="listbox"
                    aria-label={label}
                    class="menu dd-menu"
                >
                    {row(!value, anyLabel, undefined, () => {
                        onPick(null);
                        setOpen(false);
                    })}
                    {options.map((option) =>
                        row(value === option.id, option.label, option.hint, () => {
                            onPick(option.id);
                            setOpen(false);
                        }),
                    )}
                </div>
            )}
        </div>
    );
}
