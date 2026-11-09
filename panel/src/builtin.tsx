// Встроенное в пульт — как плагин: пять разделов (три на панели вкладок, «Голос», «Дом» и «Журнал» — в «Ещё»), карточка «Сейчас отвечает» и темы через тот же definePlugin, что потом у
// плагинов с шлюза.
// Экраны разделов берут данные из контекста пульта (usePanel) и помнят свою вкладку (useSectionTab).

import { IconGear, IconHome, IconJournal, IconLinks, IconPackage, IconSparkle, IconVoice } from './icons';
import { usePanel, useSectionTab } from './shell/context';
import { definePlugin } from './shell/registry';
import { GLASS, MINIMAL, MINIMAL_PANEL, PROSTOR } from './shell/themes';
import { HomeScreen } from './screens/home';
import { HouseScreen } from './screens/house';
import { JournalScreen } from './screens/journal';
import { SettingsScreen } from './screens/settings';
import { NowAnsweringCard } from './screens/characters';
import { MindScreen, MindTab, SettingsTab, isMindTab } from './screens/mind';
import { ModelsScreen, ModelsTab } from './screens/models';
import { PluginsScreen, PluginsTab } from './screens/plugins-section';
import { useOfferedVersion } from './updates';

/** Ссылка дашборда: в «Мозг» или в «Голос», сразу на нужную вкладку. */
export const settingsPlace = (place: SettingsTab): [string, string] => [isMindTab(place) ? 'mind' : 'models', place];

function HomeSection() {
    const panel = usePanel();
    return (
        <HomeScreen
            health={panel.health}
            debug={panel.debug}
            catalog={panel.catalog}
            session={panel.session}
            drawing={panel.drawing}
            online={panel.online}
            trip={panel.trip}
            skew={panel.feed.skew}
            version={panel.version}
            recent={panel.recent}
            banner={panel.banner}
            noStation={panel.noStation}
            onConnect={panel.openConnect}
            onOpenJournal={() => panel.go('journal')}
            onOpenSettings={(place) => panel.go(...settingsPlace(place))}
            onOpenHouse={(connector) => panel.openHouse(connector)}
        />
    );
}

function MindSection() {
    const panel = usePanel();
    const [tab, setTab] = useSectionTab<MindTab>('mind', 'brain');
    return <MindScreen health={panel.health} catalog={panel.catalog} tab={tab} onTab={setTab} />;
}

function ModelsSection() {
    const [tab, setTab] = useSectionTab<ModelsTab>('models', 'voice');
    return <ModelsScreen tab={tab} onTab={setTab} />;
}

function HouseSection() {
    const panel = usePanel();
    const [tab, setTab] = useSectionTab<string>('house', 'connectors');
    return (
        <HouseScreen
            catalog={panel.catalog}
            tab={tab}
            onTab={setTab}
            onOpenSettings={(place) => panel.go(...settingsPlace(place))}
            openConnector={panel.openConnector}
            onConnectorOpened={panel.connectorOpened}
        />
    );
}

function SettingsSection() {
    const panel = usePanel();
    const [tab, setTab] = useSectionTab<string>('settings', 'access');
    return <SettingsScreen catalog={panel.catalog} tab={tab} onTab={setTab} onSignOut={panel.signOut} />;
}

function PluginsSection() {
    const [tab, setTab] = useSectionTab<PluginsTab>('plugins', 'installed');
    return <PluginsScreen tab={tab} onTab={setTab} />;
}

function JournalSection() {
    const panel = usePanel();
    return <JournalScreen debug={panel.debug} catalog={panel.catalog} live={panel.feed.requests} stations={panel.stations} />;
}

/** Точка у «Дома»: шлюз нашёл новую версию. */
function UpdateDot() {
    return useOfferedVersion() ? <span class="badge-dot" role="img" aria-label="новая версия шлюза" /> : null;
}

export default definePlugin({
    id: 'core',
    sections: [
        { id: 'home', title: 'Колонка', icon: IconHome, Screen: HomeSection, order: 10 },
        { id: 'mind', title: 'Мозг', icon: IconSparkle, Screen: MindSection, order: 20 },
        // «Плагины»: что стоит, каталог и мастерская — вкладками; в панели вкладок между «Мозгом» и «Ещё».
        { id: 'plugins', title: 'Плагины', icon: IconPackage, Screen: PluginsSection, order: 25, permission: 'station.view' },
        { id: 'models', title: 'Голос', icon: IconVoice, Screen: ModelsSection, order: 30, nav: 'more' },
        { id: 'house', title: 'Дом', icon: IconLinks, Screen: HouseSection, order: 40, nav: 'more', Badge: UpdateDot },
        // «Настройки»: доступ (люди, ключи, свой вход, история) и оформление пульта — рядом с «Домом» и «Журналом».
        { id: 'settings', title: 'Настройки', icon: IconGear, Screen: SettingsSection, order: 45, nav: 'more' },
        { id: 'journal', title: 'Журнал', icon: IconJournal, Screen: JournalSection, order: 50, nav: 'more', permission: 'journal.view' },
    ],
    slots: {
        // «Сейчас отвечает: Артур» — первой из карточек главного экрана; открывает персонажа в «Мозг» → «Персонажи».
        'home.cards': [{ id: 'character', Component: NowAnsweringCard, order: 10, permission: 'station.view' }],
    },
    themes: [MINIMAL, MINIMAL_PANEL, GLASS, PROSTOR],
});
