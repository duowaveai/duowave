import { Component, ComponentChildren, RefObject } from 'preact';
import { createPortal } from 'preact/compat';
import { useEffect, useId, useLayoutEffect, useRef, useState } from 'preact/hooks';

import { IconClose, IconDots } from '../icons';
import { VIA } from '../via';
import { tr } from '../i18n';

/** Номер окна по первой отрисовке: внешнее рисуется раньше вложенного, позже открытое — позже. */
let drawn = 0;
/** Открытые окна: Esc закрывает только верхнее — с самым большим номером. */
const open = new Set<number>();

/**
 * Слово пульта колонки во фрейме окна помощника (via.ts) его странице: открыт первый шит (`open: true`) или закрыт
 * последний (`open: false`). Затемнение шита закрывает только фрейм — остальную страницу затемняет она
 * (shell/station-window.tsx).
 */
export const SHEET_MESSAGE = 'duowave:sheet';
/** Слово страницы помощника пульту во фрейме: нажали на её затемнение или Esc у неё — закрыть верхний шит. */
export const DISMISS_MESSAGE = 'duowave:dismiss';

/** Пульт колонки во фрейме окна помощника: его шиты — внутри фрейма. */
const framed = !!VIA && window.parent !== window;

/** Открытые затемнения во фрейме: слой, номер отрисовки и как закрыть нажатием мимо. */
interface Shown {
    order: number;
    confirm: boolean;
    dismiss: { current?: () => void };
}
const shown: Shown[] = [];

/** Сказать странице помощника, есть ли во фрейме шит. Её уже нет (окно закрыли) — некому. */
function tellHelper(isOpen: boolean): void {
    try {
        window.parent.postMessage({ type: SHEET_MESSAGE, open: isOpen }, location.origin);
    } catch {
        // некому сказать
    }
}

/** Страница помощника просит закрыть верхний шит: вопрос ask() над шитом, иначе позже открытый. */
function onHelperMessage(event: MessageEvent): void {
    if (event.source !== window.parent || event.origin !== location.origin) return;
    if ((event.data as { type?: unknown } | null)?.type !== DISMISS_MESSAGE) return;
    let top: Shown | null = null;
    for (const entry of shown) {
        const above = !top || (entry.confirm !== top.confirm ? entry.confirm : entry.order > top.order);
        if (above) top = entry;
    }
    top?.dismiss.current?.();
}

/** Фрейм уходит (перезагрузка, другой адрес): его шитов больше нет — затемнение снаружи не должно остаться. */
function onPageHide(): void {
    tellHelper(false);
}

// Новый пульт во фрейме начинает без шитов: если прежний ушёл, не сказав, затемнение снаружи снимается сейчас.
if (framed) tellHelper(false);

/**
 * Затемнение открылось во фрейме: первое — сказать странице помощника и слушать её; последнее закрылось — снова.
 * Говорится в том же кадре, что рисуется шит (useLayoutEffect), иначе страница кадр-другой затемнена не так, как фрейм.
 * «Шитов нет» — после всех правок этого кадра (микрозадача): шит, который другой сменил за один рендер (карточка
 * каталога → карточка поставленного персонажа), не снимает затемнение снаружи даже на кадр.
 */
function useHelperSees(layer: Layer, onDismiss?: () => void): void {
    const order = useRef(0);
    if (!order.current) order.current = ++drawn;
    const dismiss = useRef(onDismiss);
    dismiss.current = onDismiss;
    useLayoutEffect(() => {
        if (!framed) return;
        const entry: Shown = { order: order.current, confirm: layer === 'confirm', dismiss };
        shown.push(entry);
        if (shown.length === 1) {
            window.addEventListener('message', onHelperMessage);
            window.addEventListener('pagehide', onPageHide);
            tellHelper(true);
        }
        return () => {
            shown.splice(shown.indexOf(entry), 1);
            if (shown.length === 0)
                queueMicrotask(() => {
                    if (shown.length > 0) return;
                    window.removeEventListener('message', onHelperMessage);
                    window.removeEventListener('pagehide', onPageHide);
                    tellHelper(false);
                });
        };
    }, []);
}

/**
 * Окно поверх экрана: фокус уходит внутрь — на отмеченное data-autofocus, на выбранное, иначе на первое доступное, —
 * Esc закрывает, а при закрытии фокус возвращается туда, откуда окно открыли. Окна друг над другом (вопрос ask() над
 * шитом или над «+») Esc закрывает по одному, начиная с верхнего; Esc, который уже забрал кто-то внутри (список,
 * меню), окно не закрывает.
 */
export function useDialogFocus(panel: RefObject<HTMLElement>, onClose: () => void): void {
    const closeRef = useRef(onClose);
    closeRef.current = onClose;
    const order = useRef(0);
    if (!order.current) order.current = ++drawn;
    useEffect(() => {
        const self = order.current;
        open.add(self);
        const opener = document.activeElement instanceof HTMLElement ? document.activeElement : null;
        const root = panel.current;
        const target =
            root?.querySelector<HTMLElement>('[data-autofocus]:not([disabled])') ??
            root?.querySelector<HTMLElement>('[aria-checked="true"]:not([disabled])') ??
            root?.querySelector<HTMLElement>('button:not([disabled]), input:not([disabled])');
        target?.focus();
        const onKey = (event: KeyboardEvent) => {
            if (event.key !== 'Escape' || event.defaultPrevented || Math.max(...open) !== self) return;
            event.preventDefault();
            closeRef.current();
        };
        window.addEventListener('keydown', onKey);
        return () => {
            window.removeEventListener('keydown', onKey);
            open.delete(self);
            opener?.focus();
        };
    }, []);
}

/**
 * Слой окна. Порядок слоёв пульта (ui/desk.css): окно раздела 15 < док 18 < верхняя строка 20 < «Ещё» 21 < окно на
 * телефоне 25 < «+», «Колонки», разделы на телефоне 30 < шиты 40 < вопросы ask() 50. Окно колонки у помощника, пока в
 * его фрейме открыт шит, — 41: над затемнением страницы (ui/glass.css, .frame-scrim), под её вопросами.
 */
export type Layer = 'sheet' | 'confirm';

interface FailedProps {
    /** Класс затемнения — того же слоя, что у упавшего окна. */
    backdrop: string;
    /** Закрыть упавшее окно; без него оно просто прячется. */
    onClose?: () => void;
    children: ComponentChildren;
}

/**
 * Граница ошибок окна поверх пульта. Портал рисует его отдельным корнем Preact, и ошибка внутри (в эффекте, после
 * ответа API) до границ раздела или плагина снаружи не доходит: без своей границы она остановила бы пульт. Упавшее
 * окно говорит об этом коротко и закрывается, пульт работает дальше.
 */
class OverlayBoundary extends Component<FailedProps, { failed: boolean; closed: boolean }> {
    state = { failed: false, closed: false };
    private button: HTMLButtonElement | null = null;

    static getDerivedStateFromError(): { failed: boolean } {
        return { failed: true };
    }

    componentDidCatch(error: unknown): void {
        console.error('Пульт: окно поверх упало и закрыто', error);
    }

    componentDidUpdate(_: FailedProps, before: { failed: boolean }): void {
        if (this.state.failed && !before.failed) this.button?.focus();
    }

    render() {
        if (!this.state.failed) return this.props.children;
        if (this.state.closed) return null;
        const close = () => {
            this.setState({ closed: true });
            this.props.onClose?.();
        };
        return (
            <div
                class={this.props.backdrop}
                onClick={(event) => {
                    if (event.target === event.currentTarget) close();
                }}
            >
                <section role="alertdialog" aria-modal="true" aria-label={tr.ui.sheetFailedTitle} class="sheet glass-strong dash-sheet">
                    <h2 class="t-title m-0">{tr.ui.sheetFailedTitle}</h2>
                    <p class="t-meta m-0">{tr.ui.sheetFailedAbout}</p>
                    <button
                        ref={(node) => {
                            this.button = node;
                        }}
                        class="dash-done"
                        onClick={close}
                    >
                        {tr.ui.close}
                    </button>
                </section>
            </div>
        );
    }
}

/**
 * Затемнение и окно поверх пульта. Рисуется в конце <body>, а не там, где его открыли: у свободного стола и у окон
 * разделов бывает transform, у стекла — backdrop-filter, и position: fixed внутри них держался бы за них, а не за
 * экран. Контекст пульта (Preact) проходит и сюда. Нажатие на само затемнение — onDismiss. Окно упало — вместо него
 * короткое «не открылось» с «Закрыть» (onClose, без него — onDismiss). В окне колонки у помощника (фрейм) затемнение
 * закрывает только фрейм: страница помощника узнаёт о нём (SHEET_MESSAGE) и затемняет остальное, а нажатие на её
 * затемнение приходит сюда тем же onDismiss.
 */
export function Overlay({
    layer = 'sheet',
    scrim,
    onDismiss,
    onClose,
    children,
}: {
    layer?: Layer;
    /** Дополнительный класс затемнения. */
    scrim?: string;
    onDismiss?: () => void;
    /** Закрыть окно совсем: у упавшего — его «Закрыть», даже когда нажатие мимо окно не закрывает. */
    onClose?: () => void;
    children: ComponentChildren;
}) {
    const backdrop = `sheet-backdrop${layer === 'confirm' ? ' confirm-layer' : ''}${scrim ? ` ${scrim}` : ''}`;
    useHelperSees(layer, onDismiss);
    return createPortal(
        <OverlayBoundary backdrop={backdrop} onClose={onClose ?? onDismiss}>
            <div
                class={backdrop}
                onClick={(event) => {
                    if (onDismiss && event.target === event.currentTarget) onDismiss();
                }}
            >
                {children}
            </div>
        </OverlayBoundary>,
        document.body,
    );
}

/** Строка меню «⋯» в шапке окна: редкое действие. `danger` — красным, после черты. */
export interface SheetMenuItem {
    label: string;
    act: () => void;
    disabled?: boolean;
    danger?: boolean;
}

/** Вкладка окна: «Обзор · Доступ · Настройки · Версии». */
export interface SheetTab<T extends string = string> {
    id: T;
    label: string;
}

interface Props<T extends string = string> {
    title: string;
    /** Строка под заголовком: откуда, что с ним. Короткая; состояние лучше метками (`meta`). */
    about?: string;
    /** Метки под заголовком: состояние с точкой, откуда, версия. */
    meta?: ComponentChildren;
    /** Значок слева от заголовка: значок приложения или круг узла, который открыл окно. */
    icon?: ComponentChildren;
    onClose: () => void;
    children: ComponentChildren;
    /**
     * Нижняя полоса — только когда есть дело: «Есть изменения · Отменить · Сохранить» (SaveBar), «Установить», шаг
     * мастера. Главная кнопка одна, справа. «Готово» и «Закрыть» сюда не ставят: окно закрывают крестик, Esc и нажатие
     * мимо.
     */
    footer?: ComponentChildren;
    /** Редкие действия за «⋯» в шапке: перезагрузить, версии, своя копия. */
    menu?: SheetMenuItem[];
    /** Вкладки под шапкой; содержимое выбранной — в `children`. */
    tabs?: Array<SheetTab<T>>;
    tab?: T;
    onTab?: (id: T) => void;
    /** false — не закрывать нажатием мимо и Esc, без крестика: мастер с записью закрывают его собственные кнопки. */
    dismissable?: boolean;
    /** Слой: окно (по умолчанию) или вопрос ask(), который встаёт над всем. */
    layer?: Layer;
    /** Вид: обычное окно (посередине, на телефоне — лист снизу) или маленькое окно для вопроса. */
    shape?: 'window' | 'dialog';
}

/** «⋯» в шапке окна и в строке списка: меню редких действий; закрывается нажатием мимо и Esc (Esc не закрывает окно). */
export function MoreMenu({ title, items }: { title: string; items: SheetMenuItem[] }) {
    const [open, setOpen] = useState(false);
    const box = useRef<HTMLDivElement>(null);
    useEffect(() => {
        if (!open) return;
        const away = (event: PointerEvent) => {
            if (!box.current?.contains(event.target as Node)) setOpen(false);
        };
        const key = (event: KeyboardEvent) => {
            if (event.key !== 'Escape') return;
            event.preventDefault();
            setOpen(false);
        };
        window.addEventListener('pointerdown', away, true);
        window.addEventListener('keydown', key, true);
        return () => {
            window.removeEventListener('pointerdown', away, true);
            window.removeEventListener('keydown', key, true);
        };
    }, [open]);
    if (items.length === 0) return null;
    const calm = items.filter((item) => !item.danger);
    const danger = items.filter((item) => item.danger);
    const row = (item: SheetMenuItem) => (
        <button
            key={item.label}
            role="menuitem"
            class={`tab-more-item${item.danger ? ' is-danger' : ''}`}
            disabled={item.disabled}
            onClick={() => {
                setOpen(false);
                item.act();
            }}
        >
            {item.label}
        </button>
    );
    return (
        <div class="win-menu-box" ref={box}>
            <button class="win-icon-btn" aria-label={tr.ui.moreOf(title)} aria-haspopup="menu" aria-expanded={open} onClick={() => setOpen(!open)}>
                <IconDots size={18} />
            </button>
            {open && (
                <div class="win-menu menu" role="menu" aria-label={tr.ui.moreOf(title)}>
                    {calm.map(row)}
                    {danger.length > 0 && calm.length > 0 && <span class="win-menu-line" aria-hidden="true" />}
                    {danger.map(row)}
                </div>
            )}
        </div>
    );
}

/**
 * Окно пульта — одна анатомия на все (холст «Окна DuoWave: один подход», владелец 17.11): шапка (значок, имя, метки,
 * «⋯», крестик), вкладки, прокручиваемое тело, нижняя полоса только когда есть дело. На экране шире 760 точек — окно
 * посередине, окно раздела под ним прячется; на телефоне — лист снизу с ручкой. Вопрос ask() —
 * маленькое окно посередине (`shape: 'dialog'`). Нажатие мимо, Esc и крестик закрывают.
 */
export function Sheet<T extends string = string>({
    title,
    about,
    meta,
    icon,
    onClose,
    children,
    footer,
    menu,
    tabs,
    tab,
    onTab,
    dismissable = true,
    layer = 'sheet',
    shape = 'window',
}: Props<T>) {
    const panel = useRef<HTMLElement>(null);
    const labelId = useId();
    useDialogFocus(panel, dismissable ? onClose : () => undefined);
    const dialog = shape === 'dialog';
    const step = (event: KeyboardEvent, at: number) => {
        if (!tabs || !onTab) return;
        const next = event.key === 'ArrowRight' ? at + 1 : event.key === 'ArrowLeft' ? at - 1 : null;
        if (next === null) return;
        event.preventDefault();
        const entry = tabs[(next + tabs.length) % tabs.length];
        onTab(entry.id);
        (event.currentTarget as HTMLElement).parentElement?.querySelector<HTMLButtonElement>(`[data-tab="${entry.id}"]`)?.focus();
    };

    return (
        <Overlay layer={layer} scrim="dash-scrim" onDismiss={dismissable ? onClose : undefined} onClose={onClose}>
            <section
                ref={panel}
                role={dialog ? 'alertdialog' : 'dialog'}
                aria-modal="true"
                aria-labelledby={labelId}
                class={`sheet glass-strong dash-sheet win ${dialog ? 'is-dialog' : 'is-window'}`}
            >
                {!dialog && <span class="dash-grabber" aria-hidden="true" />}
                <header class="win-head">
                    {icon}
                    <div class="win-head-main">
                        <h2 id={labelId} class="t-title win-title m-0">
                            {title}
                        </h2>
                        {about && <span class="win-about">{about}</span>}
                        {meta && <div class="win-meta">{meta}</div>}
                    </div>
                    {!dialog && (menu?.length || dismissable) && (
                        <div class="win-tools">
                            {menu && <MoreMenu title={title} items={menu} />}
                            {dismissable && (
                                <button class="win-icon-btn" aria-label={tr.ui.close} onClick={onClose}>
                                    <IconClose size={18} />
                                </button>
                            )}
                        </div>
                    )}
                </header>
                {tabs && tabs.length > 1 && (
                    <div class="win-tabs">
                        <div class="win-tabs-row" role="tablist" aria-label={title}>
                            {tabs.map((entry, at) => (
                                <button
                                    key={entry.id}
                                    data-tab={entry.id}
                                    role="tab"
                                    aria-selected={entry.id === tab}
                                    tabIndex={entry.id === tab ? 0 : -1}
                                    class="win-tab"
                                    onClick={() => onTab?.(entry.id)}
                                    onKeyDown={(event) => step(event, at)}
                                >
                                    {entry.label}
                                </button>
                            ))}
                        </div>
                    </div>
                )}
                <div class="win-body">{children}</div>
                {footer && <div class="win-foot">{footer}</div>}
            </section>
        </Overlay>
    );
}

/**
 * Нижняя полоса окна с несохранённым: «Есть изменения · Отменить · Сохранить». Переключатели и выбор срабатывают
 * сразу и сюда не попадают; ждёт кнопки только текст. Ничего не изменили — полосы нет (`dirty: false` → null).
 */
export function SaveBar({
    dirty,
    busy = false,
    what,
    saveLabel = tr.ui.save,
    canSave = true,
    onRevert,
    onSave,
}: {
    dirty: boolean;
    busy?: boolean;
    /** Что изменилось, коротко: «новый токен». Без него — «Есть изменения». */
    what?: string;
    saveLabel?: string;
    canSave?: boolean;
    onRevert?: () => void;
    onSave: () => void;
}) {
    if (!dirty && !busy) return null;
    return (
        <>
            <span class="win-dirty">{what ?? tr.ui.unsaved}</span>
            <span class="win-grow" />
            {onRevert && (
                <button class="btn btn-plain" disabled={busy} onClick={onRevert}>
                    {tr.ui.revert}
                </button>
            )}
            <button class="btn btn-main" disabled={busy || !canSave} aria-busy={busy || undefined} onClick={onSave}>
                {busy && <span class="spinner spinner-sm" aria-hidden="true" />}
                {busy ? tr.ui.saving : saveLabel}
            </button>
        </>
    );
}

/** Раздел окна: подпись сверху (и короткая справа), содержимое — строками в одной рамке (`group`) или как есть. */
export function WinSection({ title, aside, group = false, children }: { title?: string; aside?: ComponentChildren; group?: boolean; children: ComponentChildren }) {
    return (
        <section class="win-sec">
            {(title || aside) && (
                <div class="win-sec-h">
                    {title && <h3 class="win-sec-title">{title}</h3>}
                    {aside && <span class="win-sec-aside">{aside}</span>}
                </div>
            )}
            {group ? <div class="win-group">{children}</div> : children}
        </section>
    );
}

/** Строка в рамке раздела: значок, имя и подпись слева, управление справа. */
export function WinRow({ icon, title, sub, children }: { icon?: ComponentChildren; title: ComponentChildren; sub?: ComponentChildren; children?: ComponentChildren }) {
    return (
        <div class="win-row">
            {icon && <span class="win-row-ico">{icon}</span>}
            <div class="win-row-main">
                <span class="win-row-t">{title}</span>
                {sub && <span class="win-row-s">{sub}</span>}
            </div>
            {children}
        </div>
    );
}

/** Место под данными, пока они идут: серые строки той же высоты, что и настоящие, — пришло, ничего не сдвинулось. */
export function SkeletonRows({ rows = 3, icon = false, label = tr.ui.loading }: { rows?: number; icon?: boolean; label?: string }) {
    return (
        <div class="win-group" role="status" aria-label={label} aria-busy="true">
            {Array.from({ length: rows }, (_, at) => (
                <div key={at} class="win-row" aria-hidden="true">
                    {icon && <span class="sk sk-ico" />}
                    <div class="win-row-main sk-lines">
                        <span class="sk sk-line" style={{ width: `${[46, 34, 52, 40][at % 4]}%` }} />
                        <span class="sk sk-line sk-thin" style={{ width: `${[68, 58, 74, 62][at % 4]}%` }} />
                    </div>
                </div>
            ))}
        </div>
    );
}
