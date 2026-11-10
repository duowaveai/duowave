// Рамка виджета на столе (docs/plans/desk.md, макеты docs/design/roles): значок, название, откуда он («колонка»,
// «журнал», «плагин»), «…» — его меню: убрать со стола, в ленте — раньше или позже, на свободном столе — шире или уже;
// у закреплённого стола эти пункты видны, но не нажимаются. Виджет, что рисует свою рамку сам (карточка плагина),
// получает только «…» в углу. Упавший виджет прячется в своей границе ошибок.

import type { ComponentChildren } from 'preact';
import { useEffect, useRef, useState } from 'preact/hooks';

import { IconDots } from '../icons';
import { Boundary } from '../shell/slot';
import type { DeskWidget } from '../shell/widgets';

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
                aria-label={`Виджет «${title}»: ещё`}
                aria-haspopup="menu"
                aria-expanded={open}
                onClick={() => setOpen(!open)}
                data-no-drag
            >
                <IconDots size={16} />
            </button>
            {open && (
                <div class="widget-menu menu" role="menu" aria-label={`Виджет «${title}»${menu.locked ? ': стол закреплён' : ''}`}>
                    {menu.locked && (
                        <p class="widget-menu-note" aria-hidden="true">
                            Стол закреплён
                        </p>
                    )}
                    {item('Раньше', menu.earlier)}
                    {item('Позже', menu.later)}
                    {item('Шире', menu.wider)}
                    {item('Уже', menu.narrower)}
                    {item('Убрать со стола', menu.hide)}
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
}: {
    widget: DeskWidget;
    menu: WidgetMenu;
    children: ComponentChildren;
    /** Свободный стол: за шапку виджет перетаскивают. */
    grip?: (event: PointerEvent) => void;
}) {
    const title = widget.title ?? widget.id;
    const body = (
        <Boundary name={widget.key} fallback={<p class="t-caption m-0">Виджет не открылся — ошибка в его коде.</p>}>
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
        <section class={`widget widget-${widget.size}`} aria-label={title}>
            <header class={`widget-head${grip ? ' widget-grip' : ''}`} onPointerDown={grip ? (event) => grip(event as PointerEvent) : undefined}>
                {Icon && <Icon size={16} />}
                <span class="widget-title">{title}</span>
                {widget.source && <span class="widget-src">{widget.source}</span>}
                <Menu title={title} menu={menu} />
            </header>
            <div class="widget-body">{body}</div>
        </section>
    );
}
