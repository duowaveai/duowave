// «Навыки» — один список (docs/plans/panel-rebuild.md, этап 3): что колонка умеет и кто на что отвечает — свой или
// Яндекс, — чего она пока не умеет (и «Научить»), подключения и программы. Глубже — память персонажей
// (screens/recall.tsx), установленное, каталог и мастерская вторым слоем, с «‹ Навыки» назад. Персонажи и темы из каталога живут у себя: в «Персонаже» и в «Оформлении».

import { useState } from 'preact/hooks';

import { can, sessionStore } from '../api';
import { tr } from '../i18n';
import { IconChevronLeft, IconChevronRight } from '../icons';
import { usePanel, useSectionTab } from '../shell/context';
import { Contribution, useSlot } from '../shell/slot';
import { timeAgo } from '../time';
import { AskSheet } from './ask';
import { PackagesTab } from './catalog';
import { Connectors } from './connectors';
import { McpSection } from './mcp';
import { settingsPlace } from './places';
import { RoutesScreen } from './routes';
import { WorkshopScreen } from './workshop';
import { MemoryScreen } from './recall';
import { OwnFrame } from './helper-pc';
import { TOP_VIA } from '../via';
import { onComputer } from '../voice-input';

const isAdmin = (): boolean => can('plugins.manage') && sessionStore.get()?.role === 'admin';

/** Вкладки второго слоя: свой экран целиком и «‹ Навыки» назад. Остальное (список, подключения) — главная страница. */
const DEEPER = new Set(['memory', 'installed', 'catalog', 'workshop']);

export function SkillsSection() {
    const [tab, setTab] = useSectionTab<string>('skills', 'list');
    // Вкладки плагинов из слота house.tabs («Дом» был их местом) — тоже второй слой.
    const extra = useSlot('house.tabs');
    const contributed = extra.find((entry) => entry.key === tab);
    if (DEEPER.has(tab) || contributed)
        return (
            <>
                <button class="deeper-back" onClick={() => setTab('list')}>
                    <IconChevronLeft size={16} />
                    {tr.skills.back}
                </button>
                {tab === 'memory' && can('memory.view') && <MemoryScreen />}
                {tab === 'installed' && <PackagesTab part="installed" />}
                {tab === 'catalog' && <PackagesTab part="catalog" />}
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
    return <SkillsList onOpen={setTab} extra={extra.map((entry) => ({ key: entry.key, title: entry.title ?? entry.id }))} />;
}

function SkillsList({ onOpen, extra }: { onOpen: (tab: string) => void; extra: Array<{ key: string; title: string }> }) {
    const panel = usePanel();
    const [asking, setAsking] = useState(false);
    const places: Array<{ key: string; title: string; sub: string; shown: boolean }> = [
        { key: 'memory', title: tr.skills.placeMemory, sub: tr.skills.placeMemorySub, shown: can('memory.view') },
        { key: 'installed', title: tr.skills.placeInstalled, sub: tr.skills.placeInstalledSub, shown: can('station.view') },
        { key: 'catalog', title: tr.skills.placeCatalog, sub: tr.skills.placeCatalogSub, shown: can('station.view') },
        { key: 'workshop', title: tr.skills.placeWorkshop, sub: tr.skills.placeWorkshopSub, shown: isAdmin() },
        ...extra.map((entry) => ({ key: entry.key, title: entry.title, sub: tr.skills.placeFromApp, shown: true })),
    ];
    return (
        <div class="skills-page">
            <section class="card glass card-list" aria-label={tr.skills.listLabel}>
                <div class="w-rows">
                    {places
                        .filter((place) => place.shown)
                        .map((place) => (
                            <button key={place.key} class="w-row w-row-btn" onClick={() => onOpen(place.key)}>
                                <span class="flex-1 min-w-0 flex flex-col">
                                    <span class="w-text">{place.title}</span>
                                    <span class="w-cap">{place.sub}</span>
                                </span>
                                <IconChevronRight size={16} />
                            </button>
                        ))}
                </div>
            </section>
            {/* Таблица «Что умеет колонка» и её «Пока не умеем» — о колонке: в режиме «Микрофон» их нет. */}
            {!onComputer() && <RoutesScreen health={panel.health} catalog={panel.catalog} title={tr.skills.speakerCan} />}
            {!onComputer() && <Unsupported />}
            {can('connectors.view') && (
                <>
                    <Connectors onOpenSettings={(place) => panel.go(...settingsPlace(place))} openId={panel.openConnector} onOpened={panel.connectorOpened} />
                    <McpSection onAsk={() => setAsking(true)} />
                    {asking && <AskSheet onClose={() => setAsking(false)} />}
                </>
            )}
        </div>
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
