// Рамка виджета на столе (docs/plans/desk.md, макеты docs/design/roles): значок, название, откуда он («колонка»,
// «журнал», «плагин»), «…» — его меню: убрать со стола, в ленте — раньше или позже, на свободном столе — шире или уже;
// у закреплённого стола эти пункты видны, но не нажимаются. Виджет, что рисует свою рамку сам (карточка плагина),
// получает только «…» в углу. Упавший виджет прячется в своей границе ошибок.

import type { ComponentChildren } from 'preact';
import { useEffect, useRef, useState } from 'preact/hooks';

import { CardLook, LookLayers, WidgetPaint } from '../cards/look';
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

function Menu({ title, menu }: { title: string; menu: WidgetMenu }) {
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
    const item = (label: string, act?: () => void) =>
        act && (
            <button
                role="menuitem"
                class="tab-more-item"
                disabled={menu.locked}
                onClick={() => {
                    setOpen(false);
                    act();
                }}
            >
                {label}
            </button>
        );
    return (
        <div class="widget-menu-box" ref={box}>
            <button
                class="widget-more"
                aria-label={tr.ui.widgetMore(title)}
                aria-haspopup="menu"
                aria-expanded={open}
                onClick={() => setOpen(!open)}
                data-no-drag
            >
                <IconDots size={16} />
            </button>
            {open && (
                <div class="widget-menu menu" role="menu" aria-label={tr.ui.widgetMenuLabel(title, !!menu.locked)}>
                    {menu.locked && (
                        <p class="widget-menu-note" aria-hidden="true">
                            {tr.ui.deskLocked}
                        </p>
                    )}
                    {item(tr.ui.moveEarlier, menu.earlier)}
                    {item(tr.ui.moveLater, menu.later)}
                    {item(tr.ui.wider, menu.wider)}
                    {item(tr.ui.narrower, menu.narrower)}
                    {item(tr.ui.removeFromDesk, menu.hide)}
                </div>
            )}
        </div>
    );
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
    const body = (
        <Boundary name={widget.key} fallback={<p class="t-caption m-0">{tr.ui.widgetFailed}</p>}>
            {children}
        </Boundary>
    );
    if (widget.bare)
        return (
            <div class={`widget-bare widget-${widget.size}`} aria-label={title}>
                {body}
                <div class="widget-bare-menu">
                    <Menu title={title} menu={menu} />
                </div>
                {grip && <span class="widget-bare-grip" aria-hidden="true" onPointerDown={(event) => grip(event as PointerEvent)} />}
            </div>
        );
    const { icon: Icon } = widget;
    return (
        <section class={`widget widget-${widget.size}${paint ? ` widget-painted look-ink-${paint.look.ink ?? 'light'}` : ''}`} aria-label={title}>
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
