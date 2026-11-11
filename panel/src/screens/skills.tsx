// «Навыки» — один список (docs/plans/panel-rebuild.md, этап 3): что колонка умеет и кто на что отвечает — свой или
// Яндекс, — чего она пока не умеет (и «Научить»), подключения и программы. Глубже — установленное, каталог и мастерская
// вторым слоем, с «‹ Навыки» назад. Персонажи и темы из каталога живут у себя: в «Персонаже» и в «Оформлении».

import { useState } from 'preact/hooks';

import { can, sessionStore } from '../api';
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
import { OwnFrame } from './helper-pc';
import { TOP_VIA } from '../via';

const isAdmin = (): boolean => can('plugins.manage') && sessionStore.get()?.role === 'admin';

/** Вкладки второго слоя: свой экран целиком и «‹ Навыки» назад. Остальное (список, подключения) — главная страница. */
const DEEPER = new Set(['installed', 'catalog', 'workshop']);

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
                    Навыки
                </button>
                {tab === 'installed' && <PackagesTab part="installed" />}
                {tab === 'catalog' && <PackagesTab part="catalog" />}
                {/* Мастерская — у компьютера: черновики пишутся там, ставятся на колонку (этап 4). */}
                {tab === 'workshop' && (TOP_VIA ? <OwnFrame page="workshop" title="Мастерская" /> : <WorkshopScreen />)}
                {contributed && (
                    <Contribution
                        entry={contributed}
                        fallback={
                            <section class="card glass" role="alert">
                                <p class="t-meta m-0">
                                    «{contributed.title ?? contributed.id}» не открылось — ошибка в плагине «{contributed.plugin}» (подробности в консоли
                                    браузера). Остальные навыки работают.
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
        { key: 'installed', title: 'Установленные навыки', sub: 'включить, выключить, настроить, обновить', shown: can('station.view') },
        { key: 'catalog', title: 'Добавить из каталога', sub: 'навыки, персонажи и темы от DuoWave и сообщества', shown: can('station.view') },
        { key: 'workshop', title: 'Сделать свой навык', sub: 'агент мастерской напишет его по вашему описанию', shown: isAdmin() },
        ...extra.map((entry) => ({ key: entry.key, title: entry.title, sub: 'от плагина', shown: true })),
    ];
    return (
        <div class="skills-page">
            <section class="card glass card-list" aria-label="Добавить и сделать">
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
            <RoutesScreen health={panel.health} catalog={panel.catalog} title="Что умеет колонка" />
            <Unsupported />
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
                    Пока не умеем
                </h2>
                <span class="t-meta">{list.length || ''}</span>
            </div>
            {list.length === 0 ? (
                <p class="w-cap m-0 pb-8">Всё, о чём спрашивали, колонка умеет.</p>
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
                                    Научить
                                </button>
                            )}
                        </div>
                    ))}
                </div>
            )}
        </section>
    );
}
