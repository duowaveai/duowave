// Рамка виджета на столе (docs/plans/desk.md, макеты docs/design/roles): значок, название, откуда он («колонка»,
// «журнал», «плагин»), «…» — его меню: убрать со стола, в ленте — раньше или позже, на свободном столе — шире или уже;
// у закреплённого стола эти пункты видны, но не нажимаются. Виджет, что рисует свою рамку сам (карточка плагина),
// получает только «…» в углу. Упавший виджет прячется в своей границе ошибок. Виджет с `hideWhileEmpty` не виден вместе
// с рамкой, пока сам не скажет, что ему есть что показать (`useWidgetEmpty`).

import type { ComponentChildren } from 'preact';
import { createContext } from 'preact';
import { useContext, useEffect, useRef, useState } from 'preact/hooks';

import { CardLook, inkOf, LookLayers, WidgetPaint } from '../cards/look';
import { IconDots } from '../icons';
import { Boundary } from '../shell/slot';
import type { DeskWidget } from '../shell/widgets';
import { tr } from '../i18n';

export interface WidgetMenu {
    hide: () => void;
    /** В ленте и столбце: на шаг раньше или позже; нет — пунктов нет. */
    earlier?: () => void;
    later?: () => void;
    /** Свободный стол: шире или уже. */
    wider?: () => void;
    narrower?: () => void;
    /** Свободный стол закреплён: пункты раскладки видны, но не нажимаются. */
    locked?: boolean;
}

/** Рамка виджета с `hideWhileEmpty` слушает, пуст ли он; у остальных рамок слушать некому. */
const WidgetEmpty = createContext<((empty: boolean) => void) | null>(null);

/**
 * Виджет говорит рамке, есть ли ему что показать: пуст — рамка не видна (если виджет так объявлен, `hideWhileEmpty`),
 * есть что — видна. Пока читает — пусть говорит «пуст»: рамка не мелькнёт у новичка, а у остальных появится с данными.
 */
export function useWidgetEmpty(empty: boolean): void {
    const tell = useContext(WidgetEmpty);
    useEffect(() => tell?.(empty), [tell, empty]);
}

/** An item of a «⋯» menu: a button, or a link opened in a new tab (`href`). */
export interface PopItem {
    label: string;
    act?: () => void;
    href?: string;
    disabled?: boolean;
}

/**
 * A «⋯» menu: opens under its button, closes on a tap elsewhere and on Esc; `note` — a line above the items. The
 * widgets' menu below and the rows of «Помощники» (screens/helpers.tsx) share it.
 */
export function PopMenu({ label, menuLabel = label, note, items }: { label: string; menuLabel?: string; note?: string; items: PopItem[] }) {
    const [open, setOpen] = useState(false);
    const box = useRef<HTMLDivElement>(null);
    useEffect(() => {
        if (!open) return;
        const away = (event: PointerEvent) => {
            if (!box.current?.contains(event.target as Node)) setOpen(false);
        };
        const key = (event: KeyboardEvent) => {
            if (event.key === 'Escape') {
                event.preventDefault();
                setOpen(false);
            }
        };
        window.addEventListener('pointerdown', away, true);
        window.addEventListener('keydown', key, true);
        return () => {
            window.removeEventListener('pointerdown', away, true);
            window.removeEventListener('keydown', key, true);
        };
    }, [open]);
    if (items.length === 0) return null;
    return (
        <div class="widget-menu-box" ref={box}>
            <button class="widget-more" aria-label={label} aria-haspopup="menu" aria-expanded={open} onClick={() => setOpen(!open)} data-no-drag>
                <IconDots size={16} />
            </button>
            {open && (
                <div class="widget-menu menu" role="menu" aria-label={menuLabel}>
                    {note && (
                        <p class="widget-menu-note" aria-hidden="true">
                            {note}
                        </p>
                    )}
                    {items.map((item) =>
                        item.href ? (
                            <a key={item.label} role="menuitem" class="tab-more-item" href={item.href} target="_blank" rel="noreferrer" onClick={() => setOpen(false)}>
                                {item.label}
                            </a>
                        ) : (
                            <button
                                key={item.label}
                                role="menuitem"
                                class="tab-more-item"
                                disabled={item.disabled}
                                onClick={() => {
                                    setOpen(false);
                                    item.act?.();
                                }}
                            >
                                {item.label}
                            </button>
                        ),
                    )}
                </div>
            )}
        </div>
    );
}

/** The widget's «⋯»: earlier, later, wider, narrower — those its place has — and «Убрать со стола»; a locked desk greys them. */
function Menu({ title, menu }: { title: string; menu: WidgetMenu }) {
    const items: PopItem[] = [];
    const all = [
        [tr.ui.moveEarlier, menu.earlier],
        [tr.ui.moveLater, menu.later],
        [tr.ui.wider, menu.wider],
        [tr.ui.narrower, menu.narrower],
        [tr.ui.removeFromDesk, menu.hide],
    ] as const;
    for (const [label, act] of all) if (act) items.push({ label, act, disabled: menu.locked });
    return <PopMenu label={tr.ui.widgetMore(title)} menuLabel={tr.ui.widgetMenuLabel(title, !!menu.locked)} note={menu.locked ? tr.ui.deskLocked : undefined} items={items} />;
}

export function WidgetFrame({
    widget,
    menu,
    children,
    grip,
    fresh,
}: {
    widget: DeskWidget;
    menu: WidgetMenu;
    children: ComponentChildren;
    /** Свободный стол: за шапку виджет перетаскивают. */
    grip?: (event: PointerEvent) => void;
    /** Сел на стол из «Примерки» меньше суток назад: метка «новый». */
    fresh?: boolean;
}) {
    const title = widget.title ?? widget.id;
    // Карточка с видом (look) красит весь виджет вместе с шапкой.
    const [paint, setPaint] = useState<{ app: string; look: CardLook } | null>(null);
    // Пустой виджет с `hideWhileEmpty` не виден, но отрисован: он читает своё и скажет, когда появится что показать.
    const [empty, setEmpty] = useState(!!widget.hideWhileEmpty);
    const hidden = !!widget.hideWhileEmpty && empty;
    const body = (
        <Boundary name={widget.key} fallback={<p class="t-caption m-0">{tr.ui.widgetFailed}</p>}>
            <WidgetEmpty.Provider value={widget.hideWhileEmpty ? setEmpty : null}>{children}</WidgetEmpty.Provider>
        </Boundary>
    );
    if (widget.bare)
        return (
            <div class={`widget-bare widget-${widget.size}`} aria-label={title} hidden={hidden}>
                {body}
                <div class="widget-bare-menu">
                    <Menu title={title} menu={menu} />
                </div>
                {grip && <span class="widget-bare-grip" aria-hidden="true" onPointerDown={(event) => grip(event as PointerEvent)} />}
            </div>
        );
    const { icon: Icon } = widget;
    return (
        <section class={`widget widget-${widget.size}${paint ? ` widget-painted look-ink-${inkOf(paint.look).ink}` : ''}`} aria-label={title} hidden={hidden}>
            {paint && <LookLayers app={paint.app} look={paint.look} />}
            {fresh && <span class="widget-fresh">{tr.ui.fresh}</span>}
            <header class={`widget-head${grip ? ' widget-grip' : ''}`} onPointerDown={grip ? (event) => grip(event as PointerEvent) : undefined}>
                {Icon && <Icon size={16} />}
                <span class="widget-title">{title}</span>
                {widget.source && <span class="widget-src">{widget.source}</span>}
                <Menu title={title} menu={menu} />
            </header>
            <div class="widget-body">
                <WidgetPaint.Provider value={setPaint}>{body}</WidgetPaint.Provider>
            </div>
        </section>
    );
}
