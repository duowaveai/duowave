import type { ComponentChildren } from 'preact';

/** Что у карточки сказать о себе: подсказка, предупреждение (можно пройти) или ошибка последнего нажатия. */
export interface CardNote {
    notice?: string;
    warning?: string;
    error?: string;
}

/** Одна карточка: слева кто это, справа чем управлять, под ним — её собственные слова. Наведение — только рамка. */
export function Card({
    title,
    badge,
    description,
    meta,
    note,
    action,
    children,
    icon,
    pills,
}: {
    title: string;
    badge?: string;
    description: string;
    meta?: string;
    note?: CardNote;
    action: ComponentChildren;
    children?: ComponentChildren;
    /** Круглый значок слева: знак сервиса у провайдеров и серверов. */
    icon?: ComponentChildren;
    /** Плашки после имени: где работает (дома, облако), отвечает ли сейчас. */
    pills?: Array<{ text: string; cls: string }>;
}) {
    return (
        <div class="voice-card flex flex-col gap-8 bt-divider py-12 px-0 r-12 my-0 mx-n8 pl-8 pr-8">
            <div class="flex items-center gap-12">
                {icon}
                <div class="flex-1 min-w-0 flex flex-col gap-2">
                    <span class="fw-700 flex gap-8 items-center flex-wrap">
                        {title}
                        {badge && <span class="pill pill-agent">{badge}</span>}
                        {pills?.map((pill) => (
                            <span key={pill.text} class={`pill ${pill.cls}`}>
                                {pill.text}
                            </span>
                        ))}
                        {meta && <span class="t-caption">{meta}</span>}
                    </span>
                    <span class="t-caption">{description}</span>
                </div>
                <div class="flex-none flex items-center gap-8">{action}</div>
            </div>
            {note?.notice && (
                <span class="t-caption c-accent-text">
                    {note.notice}
                </span>
            )}
            {note?.warning && (
                <span class="t-caption c-pill-yandex-fg" role="status">
                    {note.warning}
                </span>
            )}
            {note?.error && (
                <span class="t-caption c-dot-offline" role="alert">
                    {note.error}
                </span>
            )}
            {children}
        </div>
    );
}

/** Кнопка-пилюля справа: управляет карточкой. */
export function Action({
    on,
    label,
    tone = 'off',
    busy = false,
    disabled = false,
    onClick,
}: {
    on?: boolean;
    label: string;
    tone?: 'off' | 'on' | 'yandex';
    /** Эта кнопка сама занята: крутится спиннер. */
    busy?: boolean;
    /** Нажать пока нельзя (ждём другое): кнопка приглушена, без спиннера. */
    disabled?: boolean;
    onClick: () => void;
}) {
    const cls = on || tone === 'on' ? 'pill pill-own' : tone === 'yandex' ? 'pill pill-yandex' : 'pill pill-off';
    const resting = disabled && !busy;
    return (
        <button
            class={`${cls} py-8 px-14 min-h-36 inline-flex items-center gap-6 ${busy || disabled ? 'cursor-default' : 'cursor-pointer'}${resting ? ' opacity-45' : ''}`}
            disabled={busy || disabled}
            onClick={onClick}
        >
            {busy && <span class="spinner spinner-sm" aria-hidden="true" />}
            {label}
        </button>
    );
}
