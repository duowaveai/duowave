import { Component, ComponentChildren, RefObject } from 'preact';
import { createPortal } from 'preact/compat';
import { useEffect, useId, useLayoutEffect, useRef } from 'preact/hooks';

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
    /** Слой: шит (по умолчанию) или вопрос ask(), который встаёт над всем. */
    layer?: Layer;
}

/**
 * Шит из макета дашборда: затемнение на весь экран, заголовок, содержимое и «Готово». На телефоне — нижний лист с
 * ручкой, на экране шире 760 точек — окно посередине (ui/glass.css). Выше экрана не растёт: содержимое прокручивается
 * внутри. Нажатие мимо закрывает.
 */
export function Sheet({ title, about, icon, onClose, children, footer, dismissable = true, layer = 'sheet' }: Props) {
    const panel = useRef<HTMLElement>(null);
    const labelId = useId();
    useDialogFocus(panel, dismissable ? onClose : () => undefined);

    return (
        <Overlay layer={layer} scrim="dash-scrim" onDismiss={dismissable ? onClose : undefined} onClose={onClose}>
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
                        {tr.ui.done}
                    </button>
                )}
            </section>
        </Overlay>
    );
}
