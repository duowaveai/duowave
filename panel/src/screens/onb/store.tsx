// Первые шаги → «MagicPlace» (холст «Онбординг»: MagicPlace, MagicPlaceStation). Слева — что такое MagicPlace, справа —
// вкладки Приложения · Персонажи · Голоса · Характеры с плитками. Приложения и персонажи — из настоящего каталога
// (GET /api/v1/packages/catalog), «Поставить» ставит пакет (usePackageActions, как в MagicPlace). Голосов и характеров
// в каталоге нет: там плитки только для вида, с меткой «пример», без кнопки. Приложения и персонажи — только из каталога,
// по его записям (id, название, описание, картинка, где работает): ни названий, ни описаний конкретных пакетов в коде и
// текстах пульта (замечание 14 теста 19.11, владелец: «магазин на первых шагах должен только по id работать»).
// «Сделать» ведёт туда, где это делают: мастерская, лаборатория голосов, свой характер.
// «Открыть MagicPlace» — вторая кнопка внизу рамки.

import type { ComponentChildren } from 'preact';
import { useState } from 'preact/hooks';

import { CatalogEntry, installApi } from '../../api';
import { tr, useLanguage } from '../../i18n';
import { goesHere, kindOf, placeTitle, recommendedHere, shownAlone, wordOfEntry } from '../../magicplace/filters';
import { downloadLines } from '../../magicplace/section';
import { usePanel } from '../../shell/context';
import { useMediaUrl } from '../../ui/media';
import { canManage, catalogStatus, isBlocked, isInstalled, useCatalog, usePackageActions } from '../catalog';
import { useOnboarding, useStepNav } from './nav';
import '../../ui/onb-store.css';

type TabId = 'apps' | 'chars' | 'voices' | 'manners';
const TABS: TabId[] = ['apps', 'chars', 'voices', 'manners'];

type IconName = 'Wave' | 'Smile' | 'Plus';
type Example = keyof typeof tr.onbStore.examples;

/** Сколько пакетов каталога показать на вкладке; остальное — в MagicPlace. */
const SHOW = 5;
const SHOW_CHARS = 6;

/** Значки плиток холста (24×24, обводка — .ico). */
const ICONS: Record<IconName, ComponentChildren> = {
    Wave: <path d="M4 10v4M8 7v10M12 4v16M16 7v10M20 10v4" />,
    Smile: (
        <>
            <circle cx="12" cy="12" r="8.5" />
            <path d="M9 12.5c1.4 2 4.6 2 6 0" />
        </>
    ),
    Plus: <path d="M12 5v14M5 12h14" />,
};

/** Плитка: значок (картинка пакета, значок холста или буква), название с меткой, строки, кнопка. */
interface TileModel {
    key: string;
    title: string;
    sub: string;
    lines?: string[];
    ico: string;
    icon?: IconName;
    glyph?: string;
    image?: string | null;
    chip?: { text: string; cls: string };
    /** Где работает — меткой рядом с названием (замечание 17 теста 19.11): компьютер (Турбо) или колонка. */
    place?: { text: string; cls: string };
    button?: { label: string; light?: boolean; disabled?: boolean; onClick?: () => void };
    note?: { text: string; bad?: boolean };
}

/** Цвет плитки пакета каталога — по кругу; значок — его картинка из каталога, иначе первая буква названия. */
const APP_CYCLE = ['g-cool', 'g-violet', 'g-warm', 'g-black', 'g-teal', 'g-lime'];
const CHAR_CYCLE = ['g-cool', 'g-violet', 'g-warm', 'g-black', 'g-warm', 'g-violet'];

const glyphOf = (title: string): string => Array.from(title.trim())[0]?.toUpperCase() ?? '•';

/** Пакет годится в первые шаги: встанет здесь, не скрыт лентой (или уже стоит), не слово-активатор (п. 65: оно — с персонажем). */
const visible = (entry: CatalogEntry): boolean => goesHere(entry) && shownAlone(entry) && (!entry.unlisted || isInstalled(entry));

/** Рекомендованное — первым, остальное — как в ленте. */
const byRecommended = (list: CatalogEntry[]): CatalogEntry[] => [...list.filter(recommendedHere), ...list.filter((entry) => !recommendedHere(entry))];

export function StoreStep() {
    const lang = useLanguage();
    const panel = usePanel();
    const { leave } = useOnboarding();
    // У компьютера-помощника приложения дома ставятся у центра, через проход: его каталог, его «Поставить» (п. 46).
    const station = panel.role.role === 'helper' ? panel.role.station?.key : undefined;
    const catalog = useCatalog(station);
    const actions = usePackageActions(() => void catalog.reload(), station);
    const media = installApi(station).mediaPath;
    const [tab, setTab] = useState<TabId>('apps');

    // У помощника MagicPlace дома — центра: его окно через проход, где видны и приложения для колонки (п. 46).
    const openStore = (): void => {
        if (!station) return leave('magicplace');
        leave();
        panel.go('magicplace', undefined, undefined, { on: 'station' });
    };
    useStepNav({ secondary: { label: tr.onbStore.open, onClick: openStore } }, [lang, station]);

    /** Уйти с первых шагов в раздел пульта (и на его вкладку). */
    const goTo = (section: string, sub?: string, item?: string): void => {
        leave(section);
        if (sub) panel.go(section, sub, item);
    };

    const examples = tr.onbStore.examples;
    const example = (id: Example, ico: string, look: { icon?: IconName; glyph?: string }, chip = tr.onbStore.chipExample, cls = 'chip-out'): TileModel => ({
        key: `example:${id}`,
        title: examples[id].t,
        sub: examples[id].s,
        ico,
        ...look,
        chip: { text: chip, cls },
    });
    const make = (id: Example, onClick: () => void): TileModel => ({
        key: `make:${id}`,
        title: examples[id].t,
        sub: examples[id].s,
        ico: 'g-black',
        icon: 'Plus',
        button: { label: tr.onbStore.make, onClick },
    });

    /** Плитка пакета каталога: метка по состоянию, кнопка — поставить, обновить, включить или «Стоит ✓». */
    const entryTile = (entry: CatalogEntry, at: number): TileModel => {
        const installed = isInstalled(entry);
        const manage = canManage(entry.kind);
        const word = entry.kind === 'character' ? wordOfEntry(entry, catalog.view?.packages ?? []) : null;
        const busy = actions.busy[entry.id];
        const runs = entry.runs ?? [];
        const chip =
            entry.state === 'update'
                ? { text: tr.onbStore.chipUpdate, cls: 'chip-agent' }
                : recommendedHere(entry) && !installed
                  ? { text: tr.onbStore.chipRecommended, cls: 'chip-lime' }
                  : runs.includes('pc') && runs.includes('station')
                    ? { text: tr.onbStore.chipBoth, cls: '' }
                    : undefined;
        let button: TileModel['button'];
        if (manage && entry.state === 'update')
            button = {
                label: busy === 'update' ? tr.onbStore.installing : tr.onbStore.update,
                disabled: !!busy,
                onClick: () => void actions.update(entry.id, entry.title),
            };
        else if (manage && installed && entry.on === false)
            button = { label: tr.onbStore.switchOn, disabled: !!busy, onClick: () => void actions.switchOn(entry) };
        else if (installed) button = { label: tr.onbStore.installed, light: true, disabled: true };
        else if (manage && !isBlocked(entry))
            button = {
                label: busy === 'install' ? tr.onbStore.installing : tr.onbStore.install,
                disabled: !!busy,
                onClick: () => void actions.install(entry),
            };
        const result = actions.notes[entry.id];
        const note = result?.error
            ? { text: result.error, bad: true }
            : result?.warning
              ? { text: result.warning }
              : result?.notice
                ? { text: result.notice }
                : undefined;
        return {
            key: entry.id,
            title: entry.title,
            sub: entry.description ?? '',
            lines: [
                // Своё слово персонажа (п. 65): ставится с ним, отдельной плитки у слова нет.
                ...(word ? [tr.onbStore.wordLine(word)] : []),
                ...(installed ? [] : downloadLines(entry)),
            ],
            // Где работает: на колонке, на компьютере, в сети — центр у помощника и есть колонка. Наверху, меткой.
            place: entry.place
                ? (() => {
                      const where = entry.place === 'gateway' && station ? 'station' : entry.place;
                      return { text: placeTitle(where), cls: where === 'station' ? 'chip-station' : where === 'computer' ? 'chip-agent' : '' };
                  })()
                : undefined,
            ico: (entry.kind === 'character' ? CHAR_CYCLE : APP_CYCLE)[at % 6],
            glyph: glyphOf(entry.title),
            image: entry.preview?.image ? media(entry.preview.image) : null,
            chip,
            button,
            note,
        };
    };

    const packages = (catalog.view?.packages ?? []).filter(visible);
    const apps = byRecommended(packages.filter((entry) => kindOf(entry) === 'app'));
    const chars = byRecommended(packages.filter((entry) => entry.kind === 'character'));
    const loading = !catalog.view && !catalog.absent && !catalog.error;
    const status = catalogStatus(catalog);

    let tiles: TileModel[];
    let more = 0;
    if (tab === 'apps') {
        // Только каталог: что в нём есть — то и плитки; нет каталога — «Сделать своё» и строка, почему пусто.
        more = Math.max(0, apps.length - SHOW);
        tiles = [...apps.slice(0, SHOW).map(entryTile), make('own', () => goTo('skills', 'workshop'))];
    } else if (tab === 'chars') {
        more = Math.max(0, chars.length - SHOW_CHARS);
        tiles = chars.slice(0, SHOW_CHARS).map(entryTile);
    } else if (tab === 'voices') tiles = [example('vEn', 'g-teal', { icon: 'Wave' }), make('vOwn', () => goTo('character', 'voice'))];
    else
        tiles = [
            example('mShort', 'g-lime', { icon: 'Smile' }),
            example('mNanny', 'g-warm', { icon: 'Smile' }),
            example('mTeacher', 'g-cool', { icon: 'Smile' }),
            example('mCook', 'g-red', { icon: 'Smile' }),
            example('mCoach', 'g-black', { icon: 'Smile' }),
            make('mOwn', () => goTo('character', 'characters', 'own')),
        ];
    const fromCatalog = tab === 'apps' || tab === 'chars';

    return (
        <div class="onb-mp">
            <section class="onb-mp-side">
                <p class="eyebrow onb-mp-eyebrow">
                    <span class="mp">mp</span>MagicPlace
                </p>
                <h1 class="h2">
                    {tr.onbStore.title1}
                    <br />
                    {tr.onbStore.title2}
                </h1>
                <p class="small onb-mp-about">{tr.onbStore.about}</p>
                <ul class="checks">
                    <li>
                        <span class="mark mark-ok">
                            <svg class="ico ico-sm" viewBox="0 0 24 24" aria-hidden="true">
                                <path d="M12 3l7 3v6c0 4.5-3 7.5-7 9-4-1.5-7-4.5-7-9V6z" />
                            </svg>
                        </span>
                        <span class="s">{tr.onbStore.checkSigned}</span>
                    </li>
                    <li>
                        <span class="mark mark-ok">
                            <svg class="ico ico-sm" viewBox="0 0 24 24" aria-hidden="true">
                                <path d="M4 12a8 8 0 0 1 14-5.3M20 12a8 8 0 0 1-14 5.3" />
                                <path d="M18 3v4h-4M6 21v-4h4" />
                            </svg>
                        </span>
                        <span class="s">{tr.onbStore.checkUpdates}</span>
                    </li>
                    <li>
                        <span class="mark mark-ok">
                            <svg class="ico ico-sm" viewBox="0 0 24 24" aria-hidden="true">
                                <path d="M10 14a4 4 0 0 0 5.7 0l3-3a4 4 0 0 0-5.7-5.7l-1 1" />
                                <path d="M14 10a4 4 0 0 0-5.7 0l-3 3a4 4 0 0 0 5.7 5.7l1-1" />
                            </svg>
                        </span>
                        <span class="s">{tr.onbStore.checkSources}</span>
                    </li>
                </ul>
                <p class="small onb-mp-p">{tr.onbStore.ownLine}</p>
            </section>

            <section class="onb-mp-body">
                <div class="tabs" role="group" aria-label={tr.onbStore.tabsLabel}>
                    {TABS.map((id) => (
                        <button key={id} type="button" class="tab" aria-pressed={tab === id ? 'true' : 'false'} onClick={() => setTab(id)}>
                            {tr.onbStore.tabs[id]}
                        </button>
                    ))}
                </div>
                <p class="small onb-mp-p">{tr.onbStore.abouts[tab]}</p>
                {fromCatalog && status && (
                    <p class={`small onb-mp-p${status.tone === 'warning' ? ' onb-mp-warn' : ''}`} role="status">
                        {status.text}
                    </p>
                )}
                {fromCatalog && !loading && !status && (tab === 'apps' ? apps : chars).length === 0 && (
                    <p class="small onb-mp-p" role="status">
                        {tab === 'apps' ? tr.onbStore.noApps : tr.onbStore.noChars}
                    </p>
                )}
                {fromCatalog && loading ? (
                    <p class="small onb-mp-p" role="status">
                        {tr.onbStore.reading}
                    </p>
                ) : (
                    <div class="onb-mp-grid">
                        {tiles.map((tile) => (
                            <Tile key={tile.key} tile={tile} />
                        ))}
                    </div>
                )}
                {more > 0 && <p class="small onb-mp-p">{tr.onbStore.more(more)}</p>}
            </section>
        </div>
    );
}

function Tile({ tile }: { tile: TileModel }) {
    const src = useMediaUrl(tile.image);
    return (
        <div class="tile">
            <span class={`tile-ico ${tile.ico}`}>
                {src ? (
                    <img src={src} alt="" />
                ) : tile.icon ? (
                    <svg class="ico" viewBox="0 0 24 24" aria-hidden="true">
                        {ICONS[tile.icon]}
                    </svg>
                ) : (
                    tile.glyph
                )}
            </span>
            <span class="tile-body">
                <span class="tile-t">
                    {tile.title}
                    {tile.place && <span class={`chip${tile.place.cls ? ` ${tile.place.cls}` : ''}`}>{tile.place.text}</span>}
                    {tile.chip && <span class={`chip${tile.chip.cls ? ` ${tile.chip.cls}` : ''}`}>{tile.chip.text}</span>}
                </span>
                <span class="tile-s">{tile.sub}</span>
                {tile.lines?.map((line) => (
                    <span key={line} class="tile-s">
                        {line}
                    </span>
                ))}
                {tile.note && (
                    <span class={`tile-s${tile.note.bad ? ' onb-mp-bad' : ''}`} role={tile.note.bad ? 'alert' : 'status'}>
                        {tile.note.text}
                    </span>
                )}
            </span>
            {tile.button && (
                <button
                    type="button"
                    class={`dw-btn ${tile.button.light ? 'dw-btn-light' : 'dw-btn-dark'}`}
                    disabled={tile.button.disabled}
                    onClick={tile.button.onClick}
                >
                    {tile.button.label}
                </button>
            )}
        </div>
    );
}
