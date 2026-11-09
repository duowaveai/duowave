import { ComponentChildren, RefObject } from 'preact';
import { useEffect, useId, useRef } from 'preact/hooks';

/**
 * Окно поверх экрана: фокус уходит внутрь — на отмеченное data-autofocus, на выбранное, иначе на первое доступное, —
 * Esc закрывает, а при
 * закрытии фокус возвращается туда, откуда окно открыли.
 */
export function useDialogFocus(panel: RefObject<HTMLElement>, onClose: () => void): void {
    const closeRef = useRef(onClose);
    closeRef.current = onClose;
    useEffect(() => {
        const opener = document.activeElement instanceof HTMLElement ? document.activeElement : null;
        const root = panel.current;
        const target =
            root?.querySelector<HTMLElement>('[data-autofocus]:not([disabled])') ??
            root?.querySelector<HTMLElement>('[aria-checked="true"]:not([disabled])') ??
            root?.querySelector<HTMLElement>('button:not([disabled]), input:not([disabled])');
        target?.focus();
        const onKey = (event: KeyboardEvent) => {
            if (event.key === 'Escape') closeRef.current();
        };
        window.addEventListener('keydown', onKey);
        return () => {
            window.removeEventListener('keydown', onKey);
            opener?.focus();
        };
    }, []);
}

interface Props {
    title: string;
    about?: string;
    /** Круг с иконкой слева от заголовка — того же оттенка, что узел, который открыл шит. */
    icon?: ComponentChildren;
    onClose: () => void;
    children: ComponentChildren;
    /** Свой низ вместо «Готово»: кнопки шага мастера. */
    footer?: ComponentChildren;
    /** false — не закрывать нажатием мимо и Esc: мастер с записью закрывают его собственные кнопки. */
    dismissable?: boolean;
}

/**
 * Нижний шит «Стекла» из макета дашборда: затемнение, ручка, заголовок, содержимое и «Готово». Нажатие мимо
 * закрывает.
 */
export function Sheet({ title, about, icon, onClose, children, footer, dismissable = true }: Props) {
    const panel = useRef<HTMLElement>(null);
    const labelId = useId();
    useDialogFocus(panel, dismissable ? onClose : () => undefined);

    return (
        <div
            class="sheet-backdrop dash-scrim"
            onClick={(event) => {
                if (dismissable && event.target === event.currentTarget) onClose();
            }}
        >
            <section ref={panel} role="dialog" aria-modal="true" aria-labelledby={labelId} class="sheet glass-strong dash-sheet">
                <span class="dash-grabber" aria-hidden="true" />
                <div class="flex items-center gap-12">
                    {icon}
                    <div class="flex-1 min-w-0 flex flex-col">
                        <h2 id={labelId} class="t-title m-0">
                            {title}
                        </h2>
                        {about && <span class="t-meta">{about}</span>}
                    </div>
                </div>
                {children}
                {footer ?? (
                    <button class="dash-done" onClick={onClose}>
                        Готово
                    </button>
                )}
            </section>
        </div>
    );
}
