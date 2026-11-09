import { ComponentChildren } from 'preact';

export interface TabEntry<T extends string> {
    id: T;
    label: string;
    icon: ComponentChildren;
    /** Точка-метка: там что-то ждёт (новая версия шлюза). */
    dot?: boolean;
}

/**
 * Вкладки внутри раздела — стеклянная капсула во всю ширину, выбранная залита лаймом. Одна вкладка — не выбор:
 * тогда капсулы нет. Стрелки переводят выбор на соседнюю вкладку. Три на узком экране — без значков (glass.css).
 * Больше трёх (вкладки плагинов в «Доме») — капсула прокручивается вбок, на узком экране тоже без значков.
 */
export function SectionTabs<T extends string>({ label, tabs, tab, onPick }: { label: string; tabs: Array<TabEntry<T>>; tab: T; onPick: (id: T) => void }) {
    if (tabs.length < 2) return null;
    const step = (event: KeyboardEvent, at: number) => {
        const next = event.key === 'ArrowRight' ? at + 1 : event.key === 'ArrowLeft' ? at - 1 : null;
        if (next === null) return;
        event.preventDefault();
        const entry = tabs[(next + tabs.length) % tabs.length];
        onPick(entry.id);
        const list = (event.currentTarget as HTMLElement).parentElement;
        list?.querySelector<HTMLButtonElement>(`[data-tab="${entry.id}"]`)?.focus();
    };
    return (
        <div
            role="tablist"
            aria-label={label}
            class={tabs.length > 3 ? 'section-tabs section-tabs-many glass' : 'section-tabs glass'}
            data-count={tabs.length}
            style={tabs.length > 3 ? undefined : { gridTemplateColumns: `repeat(${tabs.length}, minmax(0, 1fr))` }}
        >
            {tabs.map((entry, at) => (
                <button
                    key={entry.id}
                    data-tab={entry.id}
                    role="tab"
                    class="section-tab"
                    aria-selected={tab === entry.id}
                    tabIndex={tab === entry.id ? 0 : -1}
                    onClick={() => onPick(entry.id)}
                    onKeyDown={(event) => step(event, at)}
                >
                    <span class="section-tab-icon" aria-hidden="true">
                        {entry.icon}
                    </span>
                    <span class="section-tab-label">{entry.label}</span>
                    {entry.dot && <span class="badge-dot" role="img" aria-label="новая версия шлюза" />}
                </button>
            ))}
        </div>
    );
}
