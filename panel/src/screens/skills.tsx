// «Приложения» (docs/plans/menu-scheme.md): вкладки «Мои · MagicPlace · Подключения» (screens/apps-tabs.tsx) и «Сделать
// своё». «Мои» — установленные плитками, внизу «Пока не умеем», что умеет колонка и вклады приложений; «Подключения» —
// внешние сервисы и программы (MCP). Глубже — мастерская и вклады приложений, с «‹ Приложения» назад. Персонажи и темы
// живут у себя: в «Персонаже» и в «Оформлении»; память — в «Персонаже».

import { useEffect, useState } from 'preact/hooks';

import { can, sessionStore } from '../api';
import { tr } from '../i18n';
import { IconChevronLeft, IconChevronRight } from '../icons';
import { usePanel, useSectionTab } from '../shell/context';
import { Contribution, useSlot } from '../shell/slot';
import { timeAgo } from '../time';
import { AppsTabs } from './apps-tabs';
import { AskSheet } from './ask';
import { PackagesTab } from './catalog';
import { Connectors } from './connectors';
import { McpSection } from './mcp';
import { settingsPlace } from './places';
import { RoutesScreen } from './routes';
import { WorkshopScreen } from './workshop';
import { OwnFrame } from './helper-pc';
import { TOP_VIA } from '../via';
import { onComputer } from '../voice-input';

const isAdmin = (): boolean => can('plugins.manage') && sessionStore.get()?.role === 'admin';

/** Вкладки второго слоя: свой экран целиком и «‹ Приложения» назад. */
const DEEPER = new Set(['workshop']);

export function SkillsSection() {
    const panel = usePanel();
    const [tab, setTab] = useSectionTab<string>('skills', 'installed');
    // Прежние вкладки: «Каталог» стал MagicPlace, общий список — вкладкой «Мои».
    useEffect(() => {
        if (tab === 'catalog') {
            setTab('installed');
            panel.go('magicplace');
        } else if (tab === 'memory') {
            // Память переехала в «Персонаж» (docs/plans/menu-scheme.md).
            setTab('installed');
            panel.go('character', 'memory');
        } else if (tab === 'list') setTab('installed');
    }, [tab]);
    // Вкладки приложений из слота house.tabs («Дом» был их местом) — второй слой, их строки внизу «Моих».
    const extra = useSlot('house.tabs');
    const contributed = extra.find((entry) => entry.key === tab);
    if (DEEPER.has(tab) || contributed)
        return (
            <>
                <button class="deeper-back" onClick={() => setTab('installed')}>
                    <IconChevronLeft size={16} />
                    {tr.skills.back}
                </button>
                {/* Мастерская — у компьютера: черновики пишутся там, ставятся на колонку (этап 4). */}
                {tab === 'workshop' && (TOP_VIA ? <OwnFrame page="workshop" title={tr.skills.workshop} /> : <WorkshopScreen />)}
                {contributed && (
                    <Contribution
                        entry={contributed}
                        fallback={
                            <section class="card glass" role="alert">
                                <p class="t-meta m-0">
                                    {tr.skills.contributedFailed(contributed.title ?? contributed.id, contributed.plugin)}
                                </p>
                            </section>
                        }
                    />
                )}
            </>
        );
    const connections = tab === 'connectors' && can('connectors.view');
    return (
        <div class="skills-page">
            <AppsTabs active={connections ? 'connectors' : 'mine'} />
            {connections ? <ConnectionsTab /> : <MineTab onOpen={setTab} extra={extra.map((entry) => ({ key: entry.key, title: entry.title ?? entry.id }))} />}
        </div>
    );
}

/** «Мои»: установленные плитками; ниже — что колонка умеет и чего пока нет, и вклады приложений. */
function MineTab({ onOpen, extra }: { onOpen: (tab: string) => void; extra: Array<{ key: string; title: string }> }) {
    const panel = usePanel();
    return (
        <>
            <PackagesTab />
            {!onComputer() && <Unsupported />}
            {/* Таблица «Что умеет колонка» — о колонке: в режиме «Микрофон» её нет. */}
            {!onComputer() && <RoutesScreen health={panel.health} catalog={panel.catalog} title={tr.skills.speakerCan} />}
            {extra.length > 0 && (
                <section class="card glass card-list" aria-label={tr.skills.fromApps}>
                    <div class="card-head">
                        <h2 class="t-title-xs m-0">{tr.skills.fromApps}</h2>
                    </div>
                    <div class="w-rows">
                        {extra.map((entry) => (
                            <button key={entry.key} class="w-row w-row-btn" onClick={() => onOpen(entry.key)}>
                                <span class="flex-1 min-w-0 flex flex-col">
                                    <span class="w-text">{entry.title}</span>
                                    <span class="w-cap">{tr.skills.placeFromApp}</span>
                                </span>
                                <IconChevronRight size={16} />
                            </button>
                        ))}
                    </div>
                </section>
            )}
        </>
    );
}

/** «Подключения»: внешние сервисы (Home Assistant, Telegram…) и программы для агента (MCP). */
function ConnectionsTab() {
    const panel = usePanel();
    const [asking, setAsking] = useState(false);
    return (
        <>
            <Connectors onOpenSettings={(place) => panel.go(...settingsPlace(place))} openId={panel.openConnector} onOpened={panel.connectorOpened} />
            <McpSection onAsk={() => setAsking(true)} />
            {asking && <AskSheet onClose={() => setAsking(false)} />}
        </>
    );
}

/** «Пока не умеем»: что спрашивали, а ответить было некому, — и «Научить»: агент мастерской сделает навык. */
function Unsupported() {
    const panel = usePanel();
    const list = panel.debug.unsupported;
    if (!can('journal.view')) return null;
    const teach = isAdmin();
    return (
        <section class="card glass card-list" aria-labelledby="unsupported-h">
            <div class="card-head">
                <h2 id="unsupported-h" class="t-title-xs m-0">
                    {tr.skills.unsupportedTitle}
                </h2>
                <span class="t-meta">{list.length || ''}</span>
            </div>
            {list.length === 0 ? (
                <p class="w-cap m-0 pb-8">{tr.skills.unsupportedNone}</p>
            ) : (
                <div class="w-rows">
                    {list.slice(0, 5).map((entry) => (
                        <div key={entry.at + entry.text} class="w-row">
                            <span class="flex-1 min-w-0 flex flex-col">
                                <span class="w-text">«{entry.text}»</span>
                                <span class="w-cap">
                                    {timeAgo(entry.at)} · {entry.reason}
                                </span>
                            </span>
                            {teach && (
                                <button class="w-btn" onClick={() => panel.go('skills', 'workshop', undefined, { new: 'plugin', teach: entry.text })}>
                                    {tr.skills.teach}
                                </button>
                            )}
                        </div>
                    ))}
                </div>
            )}
        </section>
    );
}
