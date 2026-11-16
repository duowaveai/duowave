// Встроенное в пульт — как плагин: разделы, виджеты стола и тема через тот же definePlugin, что у плагинов с шлюза.
// Четыре места (docs/plans/panel-rebuild.md): «Главная» — стол, колонка в середине и виджеты вокруг; «Персонаж»,
// «Навыки» и «Настройки» — screens/places.tsx. «Память» (screens/recall.tsx, docs/plans/memory.md) — страница «Приложений». Прежние разделы «Мозг», «Голос», «Дом», «Плагины» и «Журнал» лежат в них,
// а их адреса ведут на новые места (shell/links.ts). Экраны берут данные из контекста пульта (usePanel).

import { tr } from './i18n';
import { IconCode, IconFolder, IconGear, IconGlobe, IconHome, IconJournal, IconLaptop, IconList, IconMemory, IconMusic, IconPackage, IconShield, IconSmile, IconSparkle, IconTimer } from './icons';
import { definePlugin } from './shell/registry';
import { MINIMAL } from './shell/themes';
import { NowAnsweringCard } from './screens/characters';
import { CharacterSection } from './screens/character';
import { SettingsSection } from './screens/places';
import { SkillsSection } from './screens/skills';
import { MemoryWidget } from './screens/recall';
import { DeskScreen } from './screens/desk';
import { FittingSection } from './screens/fitting';
import { WorksSection } from './screens/works';
import { EgressScreen } from './screens/egress';
import { FoldersSection } from './screens/folders';
import {
    AllocWidget,
    HelperCardWidget,
    HelperWidget,
    ModeWidget,
    RecentWidget,
    UnsupportedWidget,
    WatchWidget,
    WorkshopWidget,
    WorksWidget,
} from './screens/desk-widgets';
import { MusicWidget, TimersWidget } from './screens/station-widgets';
import { ShowcaseWidget } from './screens/showcase';
import { useOfferedVersion } from './updates';
import { IconMagicPlace } from './magicplace/icon';
import { MagicPlaceSection } from './magicplace/section';
import { MagicPlaceWidget, UpdatesBadge } from './magicplace/widget';

/** Ссылка дашборда: в «Персонаж», «Навыки» или «Настройки», сразу на нужную вкладку. */
export { settingsPlace } from './screens/places';

/** Точка у «Настроек»: шлюз нашёл новую версию (её страница — «Обновления и копии»). */
function UpdateDot() {
    return useOfferedVersion() ? <span class="badge-dot" role="img" aria-label={tr.core.newGatewayVersion} /> : null;
}

/**
 * Виджеты о самой колонке — режим, её музыка и таймеры, кто сколько ответил, чего она не умеет: в режиме «Микрофон»
 * (docs/plans/mic-mode.md) колонки в пульте нет, и их нет тоже.
 */
const withStation = (health: { voiceInput?: string } | null): boolean => health?.voiceInput !== 'computer';

/** Колонка со своим шлюзом: у неё может быть помощник — его виджет есть только там. */
const onStation = (health: { placement?: { role?: string } } | null): boolean => health?.placement?.role === 'station';

export default definePlugin({
    id: 'core',
    sections: [
        { id: 'home', get title() { return tr.core.sectionHome; }, icon: IconHome, Screen: DeskScreen, order: 10 },
        { id: 'character', get title() { return tr.core.sectionCharacter; }, icon: IconSmile, Screen: CharacterSection, order: 20 },
        // «Навыки» стали «Приложениями» (docs/plans/voice-creation.md, решение 11): навык — лицо приложения.
        // Число обновлений MagicPlace — на «Приложениях»: магазин стал их вкладкой (docs/plans/menu-scheme.md).
        { id: 'skills', get title() { return tr.core.sectionApps; }, icon: IconPackage, Screen: SkillsSection, order: 30, Badge: UpdatesBadge },
        // MagicPlace — встроенный магазин (docs/plans/magicplace.md): часть пульта и шлюза, не пакет — не выключить и не
        // удалить. В доке сразу после «Приложений», с числом обновлений; окно — `#/magicplace/store/<пакет>`.
        // В доке ячейки нет (docs/plans/menu-scheme.md): вкладка «Приложений», ярлык на столе; адреса прежние.
        { id: 'magicplace', get title() { return tr.magicplace.title; }, icon: IconMagicPlace, Screen: MagicPlaceSection, order: 32, nav: 'link', permission: 'station.view', Badge: UpdatesBadge },
        // Домашняя папка DuoWave (docs/plans/shared-folders.md): что в ней, что подключено, кто что видит.
        // Ячейки в доке нет: «Папки» — приложение, открываются ярлыком со стола, плиткой и голосом.
        { id: 'folders', get title() { return tr.core.sectionFolders; }, icon: IconFolder, Screen: FoldersSection, order: 35, nav: 'link', permission: 'station.view' },
        { id: 'settings', get title() { return tr.core.sectionSettings; }, icon: IconGear, Screen: SettingsSection, order: 40, Badge: UpdateDot },
        // «Примерка» черновика (docs/plans/voice-creation.md): только по ссылке `#/fitting/<черновик>` — её открывают
        // плитка, полоса голоса и колонка («примерить»); ячейки в доке нет.
        { id: 'fitting', get title() { return tr.core.sectionFitting; }, icon: IconSparkle, Screen: FittingSection, order: 90, nav: 'link', permission: 'plugins.manage' },
        // «Работы» (docs/plans/router-and-works.md, решение 17): что идёт, что ждёт и почему, что спрашивает, недавние; по
        // ссылке `#/works/<работа>` — сама работа. Ячейки в доке нет: туда ведут виджет «Работы» на столе и плашка «Делаю
        // виджет». Администратору: в ней модели и деньги.
        { id: 'works', get title() { return tr.router.title; }, icon: IconList, Screen: WorksSection, order: 92, nav: 'link', permission: 'settings.edit' },
        // «Выход в интернет» (docs/plans/own-server.md): страница «Настроек», а по ссылке `#/egress` — сама; туда ведёт окно
        // приложения «VPN» (`go("egress")`).
        { id: 'egress', get title() { return tr.egress.title; }, icon: IconGlobe, Screen: EgressScreen, order: 94, nav: 'link', permission: 'connectors.view' },
    ],
    slots: {
        // Виджеты стола: у шлюза — о колонке и её ответах, у помощника — о том, что он ей даёт.
        'desk.widgets': [
            // Витрина (docs/plans/start-desk.md): под сценой, шире её — три карточки «скажи — и сделается»; сначала «Сделайте
            // Алису умнее». Только на новом стартовом столе; попробовали всё или скрыли — уходит.
            { id: 'showcase', get title() { return tr.showcase.label; }, icon: IconSparkle, Component: ShowcaseWidget, order: 5, size: 'large', bare: true, under: true, wide: true, roles: ['gateway'], start: true, initial: false, hideWhileEmpty: true },
            { id: 'mode', get title() { return tr.core.widgetMode; }, icon: IconSparkle, Component: ModeWidget, order: 10, size: 'large', get source() { return tr.core.sourceStation; }, roles: ['gateway'], available: withStation },
            { id: 'music', get title() { return tr.core.widgetMusic; }, icon: IconMusic, Component: MusicWidget, order: 12, start: true, hideWhileEmpty: true, size: 'large', bare: true, permission: 'station.view', available: withStation },
            { id: 'timers', get title() { return tr.core.widgetTimers; }, icon: IconTimer, Component: TimersWidget, order: 14, start: true, hideWhileEmpty: true, size: 'small', get source() { return tr.core.sourceStation; }, permission: 'station.view', available: withStation },
            { id: 'character', get title() { return tr.core.widgetCharacter; }, Component: NowAnsweringCard, order: 15, size: 'small', bare: true, roles: ['gateway'], permission: 'station.view' },
            // Память: стартовый экран простой — на стол её ставят сами, через «Новый виджет» (владелец, 12.11).
            { id: 'memory', get title() { return tr.core.widgetMemory; }, icon: IconMemory, Component: MemoryWidget, order: 16, size: 'small', get source() { return tr.core.sourceHome; }, roles: ['gateway'], permission: 'memory.view', initial: false },
            // MagicPlace на столе: знак и одна строка — обновления, новое, рекомендуем; убрать можно, магазин остаётся в доке.
            // Сразу на столе не стоит (владелец, 15.11): MagicPlace — ярлык; виджет со строкой новостей — по желанию.
            { id: 'magicplace', get title() { return tr.magicplace.title; }, icon: IconMagicPlace, Component: MagicPlaceWidget, order: 18, size: 'small', bare: true, permission: 'station.view', initial: false },
            // «Работы»: сколько идёт и сколько ждёт ответа, по строке на работу; нажатие — страница «Работы» (решение 17).
            // Пока работ не было ни одной, его не видно: у новичка пустой виджет — шум (владелец, 16.11), появится сам.
            { id: 'works', get title() { return tr.router.widgetTitle; }, icon: IconList, Component: WorksWidget, order: 19, start: true, size: 'large', get source() { return tr.core.sourceHome; }, permission: 'settings.edit', hideWhileEmpty: true },
            // «Последние вопросы»: до первого вопроса не видны — подсказка «спросите что-нибудь» уже есть под колонкой.
            { id: 'recent', get title() { return tr.core.widgetRecent; }, icon: IconJournal, Component: RecentWidget, order: 20, start: true, size: 'large', get source() { return tr.core.sourceHistory; }, permission: 'journal.view', hideWhileEmpty: true },
            { id: 'helper-card', get title() { return tr.core.widgetHelperCard; }, icon: IconLaptop, Component: HelperCardWidget, order: 25, start: true, hideWhileEmpty: true, size: 'small', get source() { return tr.core.sourceComputer; }, roles: ['gateway'], available: onStation },
            { id: 'alloc', get title() { return tr.core.widgetAlloc; }, Component: AllocWidget, order: 30, size: 'large', bare: true, roles: ['gateway'], available: withStation },
            { id: 'unsupported', get title() { return tr.core.widgetUnsupported; }, icon: IconJournal, Component: UnsupportedWidget, order: 40, start: true, hideWhileEmpty: true, size: 'large', get source() { return tr.core.sourceHistory; }, roles: ['gateway'], permission: 'journal.view', available: withStation },
            { id: 'helper', get title() { return tr.core.widgetHelper; }, icon: IconLaptop, Component: HelperWidget, order: 10, start: true, hideWhileEmpty: true, size: 'large', get source() { return tr.core.sourceThisComputer; }, roles: ['helper'] },
            { id: 'watch', get title() { return tr.core.widgetWatch; }, icon: IconShield, Component: WatchWidget, order: 20, size: 'large', get source() { return tr.core.sourceThisComputer; }, roles: ['helper'], permission: 'station.view' },
            { id: 'workshop', get title() { return tr.core.widgetWorkshop; }, icon: IconCode, Component: WorkshopWidget, order: 30, size: 'large', get source() { return tr.core.sourceThisComputer; }, roles: ['helper'], permission: 'plugins.manage' },
        ],
    },
    themes: [MINIMAL],
});
