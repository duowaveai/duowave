// Раздел «Плагины»: всё о пакетах в одном месте — что стоит, общий каталог и мастерская, тремя вкладками. Раньше это
// были «Дом» → «Пакеты» и отдельный раздел «Мастерская»; старые ссылки ведут сюда (shell/links.ts). «Установлено» и
// «Каталог» видны с правом смотреть колонку, «Мастерская» — администратору: черновик становится кодом в шлюзе.

import { can, sessionStore } from '../api';
import { IconBraces, IconPackage, IconSparkle } from '../icons';
import { SectionTabs, TabEntry } from '../ui/tabs';
import { PackagesTab } from './catalog';
import { WorkshopScreen } from './workshop';

export type PluginsTab = 'installed' | 'catalog' | 'workshop';

const isAdmin = (): boolean => can('plugins.manage') && sessionStore.get()?.role === 'admin';

export function PluginsScreen({ tab, onTab }: { tab: PluginsTab; onTab: (tab: PluginsTab) => void }) {
    const tabs: Array<TabEntry<PluginsTab>> = [
        { id: 'installed', label: 'Установлено', icon: <IconPackage size={16} /> },
        { id: 'catalog', label: 'Каталог', icon: <IconSparkle size={16} /> },
        ...(isAdmin() ? [{ id: 'workshop' as const, label: 'Мастерская', icon: <IconBraces size={16} /> }] : []),
    ];
    // Вкладка, которой этому входу не видно (мастерская не администратору), — «Установлено».
    const shown = tabs.some((entry) => entry.id === tab) ? tab : 'installed';
    return (
        <>
            <SectionTabs label="Плагины" tabs={tabs} tab={shown} onPick={onTab} />
            {shown === 'installed' && <PackagesTab part="installed" />}
            {shown === 'catalog' && <PackagesTab part="catalog" />}
            {shown === 'workshop' && <WorkshopScreen />}
        </>
    );
}
