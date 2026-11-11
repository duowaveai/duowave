// Встроенное в пульт — как плагин: разделы, виджеты стола и тема через тот же definePlugin, что у плагинов с шлюза.
// Четыре места (docs/plans/panel-rebuild.md): «Главная» — стол, колонка в середине и виджеты вокруг; «Персонаж»,
// «Навыки» и «Настройки» — screens/places.tsx. Прежние разделы «Мозг», «Голос», «Дом», «Плагины» и «Журнал» лежат в них,
// а их адреса ведут на новые места (shell/links.ts). Экраны берут данные из контекста пульта (usePanel).

import { IconCode, IconGear, IconHome, IconJournal, IconLaptop, IconMusic, IconPackage, IconShield, IconSmile, IconSparkle, IconTimer } from './icons';
import { definePlugin } from './shell/registry';
import { MINIMAL } from './shell/themes';
import { NowAnsweringCard } from './screens/characters';
import { CharacterSection } from './screens/character';
import { SettingsSection } from './screens/places';
import { SkillsSection } from './screens/skills';
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

/** Ссылка дашборда: в «Персонаж», «Навыки» или «Настройки», сразу на нужную вкладку. */
export { settingsPlace } from './screens/places';

/** Точка у «Настроек»: шлюз нашёл новую версию (её страница — «Обновления и копии»). */
function UpdateDot() {
    return useOfferedVersion() ? <span class="badge-dot" role="img" aria-label="новая версия шлюза" /> : null;
}

/** Колонка со своим шлюзом: у неё может быть помощник — его виджет есть только там. */
const onStation = (health: { placement?: { role?: string } } | null): boolean => health?.placement?.role === 'station';

export default definePlugin({
    id: 'core',
    sections: [
        { id: 'home', title: 'Главная', icon: IconHome, Screen: DeskScreen, order: 10 },
        { id: 'character', title: 'Персонаж', icon: IconSmile, Screen: CharacterSection, order: 20 },
        { id: 'skills', title: 'Навыки', icon: IconPackage, Screen: SkillsSection, order: 30 },
        { id: 'settings', title: 'Настройки', icon: IconGear, Screen: SettingsSection, order: 40, Badge: UpdateDot },
    ],
    slots: {
        // Виджеты стола: у шлюза — о колонке и её ответах, у помощника — о том, что он ей даёт.
        'desk.widgets': [
            { id: 'mode', title: 'Кто отвечает', icon: IconSparkle, Component: ModeWidget, order: 10, size: 'large', source: 'колонка', roles: ['gateway'] },
            { id: 'music', title: 'Музыка', icon: IconMusic, Component: MusicWidget, order: 12, size: 'large', bare: true, permission: 'station.view' },
            { id: 'timers', title: 'Таймеры', icon: IconTimer, Component: TimersWidget, order: 14, size: 'small', source: 'колонка', permission: 'station.view' },
            { id: 'character', title: 'Сейчас отвечает', Component: NowAnsweringCard, order: 15, size: 'small', bare: true, roles: ['gateway'], permission: 'station.view' },
            { id: 'recent', title: 'Последние вопросы', icon: IconJournal, Component: RecentWidget, order: 20, size: 'large', source: 'история', permission: 'journal.view' },
            { id: 'helper-card', title: 'Компьютер-помощник', icon: IconLaptop, Component: HelperCardWidget, order: 25, size: 'small', source: 'компьютер', roles: ['gateway'], available: onStation },
            { id: 'alloc', title: 'Кто сколько ответил', Component: AllocWidget, order: 30, size: 'large', bare: true, roles: ['gateway'] },
            { id: 'unsupported', title: 'Пока не умеем', icon: IconJournal, Component: UnsupportedWidget, order: 40, size: 'large', source: 'история', roles: ['gateway'], permission: 'journal.view' },
            { id: 'helper', title: 'Помогать колонке', icon: IconLaptop, Component: HelperWidget, order: 10, size: 'large', source: 'этот компьютер', roles: ['helper'] },
            { id: 'watch', title: 'Присматриваю за колонкой', icon: IconShield, Component: WatchWidget, order: 20, size: 'large', source: 'этот компьютер', roles: ['helper'], permission: 'station.view' },
            { id: 'workshop', title: 'Мастерская', icon: IconCode, Component: WorkshopWidget, order: 30, size: 'large', source: 'этот компьютер', roles: ['helper'], permission: 'plugins.manage' },
        ],
    },
    themes: [MINIMAL],
});
