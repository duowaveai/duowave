// «+» — новый виджет на стол (docs/plans/desk.md, макет SpaceAddPanel): справа панель — готовые виджеты, встроенные и
// из плагинов, которых на столе ещё нет, и «Каталог» — за новыми плагинами. Убранный со стола виджет ставится отсюда
// снова. Сверху — просьба словами: виджет рисует агент мастерской (screens/widget-agent.tsx).

import { ComponentChildren } from 'preact';
import { useRef } from 'preact/hooks';

import { IconClose, IconPackage, IconPlus } from '../icons';
import { usePanel } from '../shell/context';
import { DeskWidget, onDesk, useDeskLayout, useWidgets } from '../shell/widgets';
import { useDialogFocus } from '../ui/sheet';

export function AddWidgetPanel({ onClose, children }: { onClose: () => void; children?: ComponentChildren }) {
    const panel = usePanel();
    const role = panel.role.role;
    const widgets = useWidgets(role, panel.health);
    const [layout, actions] = useDeskLayout(role);
    const box = useRef<HTMLElement>(null);
    useDialogFocus(box, onClose);
    const off = widgets.filter((widget) => !onDesk(widget, layout));
    const put = (widget: DeskWidget) => {
        actions.add(widget.key);
        onClose();
    };
    return (
        <div class="add-layer">
            <button class="add-scrim" aria-label="Закрыть" tabIndex={-1} onClick={onClose} />
            <aside ref={box} class="add-panel" role="dialog" aria-modal="true" aria-labelledby="add-widget-h">
                <header class="add-head">
                    <div class="flex-1 min-w-0 flex flex-col">
                        <h2 id="add-widget-h" class="add-title">
                            Новый виджет
                        </h2>
                        <span class="w-cap">голосом или текстом — или готовый</span>
                    </div>
                    <button class="desk-window-close" aria-label="Закрыть" onClick={onClose}>
                        <IconClose size={18} />
                    </button>
                </header>
                {children}
                <section class="flex flex-col gap-8" aria-labelledby="add-ready-h">
                    <h3 id="add-ready-h" class="add-sub">
                        {off.length > 0 ? 'Готовые' : 'Готовые — все уже на столе'}
                    </h3>
                    {off.length > 0 && (
                        <div class="add-list">
                            {off.map((widget) => {
                                const { icon: Icon } = widget;
                                return (
                                    <button key={widget.key} class="add-item" onClick={() => put(widget)}>
                                        <span class="add-item-icon" aria-hidden="true">
                                            {Icon ? <Icon size={18} /> : <IconPlus size={18} />}
                                        </span>
                                        <span class="flex-1 min-w-0 flex flex-col">
                                            <span class="w-strong">{widget.title ?? widget.id}</span>
                                            <span class="w-cap">
                                                {[widget.plugin === 'core' ? 'встроенный' : 'плагин', widget.size === 'large' ? 'большой' : 'маленький']
                                                    .filter(Boolean)
                                                    .join(' · ')}
                                            </span>
                                        </span>
                                        <span class="pill pill-off">Поставить</span>
                                    </button>
                                );
                            })}
                        </div>
                    )}
                    <button
                        class="add-item"
                        onClick={() => {
                            onClose();
                            panel.go('plugins', 'catalog');
                        }}
                    >
                        <span class="add-item-icon" aria-hidden="true">
                            <IconPackage size={18} />
                        </span>
                        <span class="flex-1 min-w-0 flex flex-col">
                            <span class="w-strong">Каталог</span>
                            <span class="w-cap">плагины с виджетами: погода, радио, курсы, список покупок…</span>
                        </span>
                    </button>
                </section>
                <p class="w-cap m-0">Виджет на столе можно двигать за его шапку и растягивать за угол.</p>
            </aside>
        </div>
    );
}
