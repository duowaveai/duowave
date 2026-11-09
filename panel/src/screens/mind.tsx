import { AccessCatalog, Health } from '../api';
import { IconRoutes, IconSmile, IconSparkle } from '../icons';
import { SectionTabs, TabEntry } from '../ui/tabs';
import { BrainTab } from './brain';
import { CharactersTab } from './characters';
import type { ModelsTab } from './models';
import { RoutesScreen } from './routes';

export type MindTab = 'routes' | 'brain' | 'characters';

/** Куда ведут ссылки дашборда: вкладка «Мозга» или «Голоса». */
export type SettingsTab = MindTab | ModelsTab;

export const isMindTab = (tab: SettingsTab): tab is MindTab => tab === 'routes' || tab === 'brain' || tab === 'characters';

const MIND_TABS: Array<TabEntry<MindTab>> = [
    { id: 'brain', label: 'Модели', icon: <IconSparkle size={16} /> },
    { id: 'characters', label: 'Персонажи', icon: <IconSmile size={16} /> },
    { id: 'routes', label: 'Маршруты', icon: <IconRoutes size={16} /> },
];

/** «Мозг»: какая модель думает, каким персонажем она говорит и кто отвечает на что (маршруты). */
export function MindScreen({ health, catalog, tab, onTab }: { health: Health | null; catalog: AccessCatalog | null; tab: MindTab; onTab: (tab: MindTab) => void }) {
    // Вкладка из ссылки, которой нет, — первая.
    const shown = MIND_TABS.some((entry) => entry.id === tab) ? tab : 'brain';
    return (
        <>
            <SectionTabs label="Мозг" tabs={MIND_TABS} tab={shown} onPick={onTab} />
            {shown === 'brain' ? <BrainTab /> : shown === 'characters' ? <CharactersTab /> : <RoutesScreen health={health} catalog={catalog} />}
        </>
    );
}
