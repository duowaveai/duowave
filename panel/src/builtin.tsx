// Встроенное в пульт — как плагин: разделы, виджеты стола и тема через тот же definePlugin, что у плагинов с шлюза.
// Главный раздел — стол (docs/plans/desk.md): колонка в середине, виджеты вокруг. Экраны разделов берут данные из
// контекста пульта (usePanel) и помнят свою вкладку (useSectionTab).

import { IconCode, IconGear, IconHome, IconJournal, IconLaptop, IconLinks, IconMusic, IconPackage, IconShield, IconSparkle, IconTimer, IconVoice } from './icons';
import { usePanel, useSectionTab } from './shell/context';
import { definePlugin } from './shell/registry';
import { MINIMAL } from './shell/themes';
import { HouseScreen } from './screens/house';
import { JournalScreen } from './screens/journal';
import { SettingsScreen } from './screens/settings';
import { NowAnsweringCard } from './screens/characters';
import { MindScreen, MindTab, SettingsTab, isMindTab } from './screens/mind';
import { ModelsScreen, ModelsTab } from './screens/models';
import { PluginsScreen, PluginsTab } from './screens/plugins-section';
import { DeskScreen } from './screens/desk';
import {
    AllocWidget,
    HelperCardWidget,
    HelperWidget,
    ModeWidget,
    RecentWidget,
    UnsupportedWidget,
    WatchWidget,
    WorkshopWidget,
} from './screens/desk-widgets';
import { MusicWidget, TimersWidget } from './screens/station-widgets';
import { useOfferedVersion } from './updates';

/** Ссылка дашборда: в «Мозг» или в «Голос», сразу на нужную вкладку. */
export const settingsPlace = (place: SettingsTab): [string, string] => [isMindTab(place) ? 'mind' : 'models', place];

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

/** Колонка со своим шлюзом: у неё может быть помощник — его виджет есть только там. */
const onStation = (health: { placement?: { role?: string } } | null): boolean => health?.placement?.role === 'station';

export default definePlugin({
    id: 'core',
    sections: [
        { id: 'home', title: 'Колонка', icon: IconHome, Screen: DeskScreen, order: 10 },
        { id: 'mind', title: 'Мозг', icon: IconSparkle, Screen: MindSection, order: 20 },
        // «Плагины»: что стоит, каталог и мастерская — вкладками; в доке между «Сказать» и «Ещё».
        { id: 'plugins', title: 'Плагины', icon: IconPackage, Screen: PluginsSection, order: 25, permission: 'station.view' },
        { id: 'models', title: 'Голос', icon: IconVoice, Screen: ModelsSection, order: 30, nav: 'more' },
        { id: 'house', title: 'Дом', icon: IconLinks, Screen: HouseSection, order: 40, nav: 'more', Badge: UpdateDot },
        // «Настройки»: доступ (люди, ключи, свой вход, история) и оформление пульта — рядом с «Домом» и «Журналом».
        { id: 'settings', title: 'Настройки', icon: IconGear, Screen: SettingsSection, order: 45, nav: 'more' },
        { id: 'journal', title: 'Журнал', icon: IconJournal, Screen: JournalSection, order: 50, nav: 'more', permission: 'journal.view' },
    ],
    slots: {
        // Виджеты стола: у шлюза — о колонке и её ответах, у помощника — о том, что он ей даёт.
        'desk.widgets': [
            { id: 'mode', title: 'Режим колонки', icon: IconSparkle, Component: ModeWidget, order: 10, size: 'large', source: 'колонка', roles: ['gateway'] },
            { id: 'music', title: 'Музыка', icon: IconMusic, Component: MusicWidget, order: 12, size: 'large', bare: true, permission: 'station.view' },
            { id: 'timers', title: 'Таймеры', icon: IconTimer, Component: TimersWidget, order: 14, size: 'small', source: 'колонка', permission: 'station.view' },
            { id: 'character', title: 'Сейчас отвечает', Component: NowAnsweringCard, order: 15, size: 'small', bare: true, roles: ['gateway'], permission: 'station.view' },
            { id: 'recent', title: 'Последние вопросы', icon: IconJournal, Component: RecentWidget, order: 20, size: 'large', source: 'журнал', permission: 'journal.view' },
            { id: 'helper-card', title: 'Помощник', icon: IconLaptop, Component: HelperCardWidget, order: 25, size: 'small', source: 'компьютер', roles: ['gateway'], available: onStation },
            { id: 'alloc', title: 'Кто сколько ответил', Component: AllocWidget, order: 30, size: 'large', bare: true, roles: ['gateway'] },
            { id: 'unsupported', title: 'Пока не умеем', icon: IconJournal, Component: UnsupportedWidget, order: 40, size: 'large', source: 'журнал', roles: ['gateway'], permission: 'journal.view' },
            { id: 'helper', title: 'Что даю колонке', icon: IconLaptop, Component: HelperWidget, order: 10, size: 'large', source: 'этот компьютер', roles: ['helper'] },
            { id: 'watch', title: 'Присматриваю за колонкой', icon: IconShield, Component: WatchWidget, order: 20, size: 'large', source: 'этот компьютер', roles: ['helper'], permission: 'station.view' },
            { id: 'workshop', title: 'Мастерская', icon: IconCode, Component: WorkshopWidget, order: 30, size: 'large', source: 'этот компьютер', roles: ['helper'], permission: 'plugins.manage' },
        ],
    },
    themes: [MINIMAL],
});
