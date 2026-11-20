// «Мозг» → «Персонажи»: галерея — встроенные, поставленные, «Каталог» (персонажи из ленты, ещё не поставленные) и
// последней — «Собрать своего». Персонаж = характер (промт и регуляторы тона) + голос + оформление
// (docs/plans/characters.md). Каталог здесь — тот же, что в «Дом» → «Пакеты», отфильтрованный по виду character
// (screens/catalog.tsx). Карточка персонажа: аватар и приветствие, тоны, строка голоса, «Выбрать», «Тонкая настройка»
// с полным текстом (тот же редактор и PUT /api/v1/persona), «Удалить». Голосу без модели — окно «нужна модель».
// Ссылка `#/mind/characters/<пакет>?install` (сайт каталога) открывает персонажа, а не поставленного — его пакет с
// кнопкой установки; после установки — персонажа и окно модели.

import { useEffect, useRef, useState } from 'preact/hooks';

import {
    ActiveCharacter,
    ApiError,
    CatalogEntry,
    CharacterView,
    CharactersView,
    ModelsView,
    PersonaInfo,
    api,
    installApi,
    can,
    isTight,
    needsOf,
    problemOf,
} from '../api';
import { IconChevronRight, IconPlus } from '../icons';
import { useLinkedItem, usePanel } from '../shell/context';
import { CardNote } from '../ui/cards';
import { diffText } from '../ui/diff';
import { Avatar, PlayButton, markOf } from '../ui/media';
import { Progress, useNeedsFlow, voicePill } from '../ui/needs';
import { Sheet, SkeletonRows, WinRow, WinSection } from '../ui/sheet';
import {
    CatalogEntrySheet,
    Companions,
    NoteLine,
    PackageActions,
    canManage,
    catalogOpen,
    catalogStatus,
    entryPills,
    feedLines,
    isInstalled,
    isBlocked,
    useCatalog,
    useEngines,
    usePackageActions,
} from './catalog';
import { PersonaEditor, PersonaTab } from './persona';
import { OwnVoiceRows, VoiceWizard } from './voice-lab';
import { tr } from '../i18n';
import { byMode } from '../words';

/** id «Собрать своего» в адресе: `#/mind/characters/own`. */
const OWN = 'own';

/** Крутилка в занятой кнопке. */
const spin = (on: boolean) => on && <span class="spinner spinner-sm" aria-hidden="true" />;

type Pill = { text: string; cls: string };

/** Персонаж по id из адреса: свой id (`p-6dbdfc39330b:arthur`, `standard`) или id его пакета (`p-6dbdfc39330b` — так ведёт сайт). */
function findCharacter(list: CharacterView[], item: string): CharacterView | undefined {
    return list.find((entry) => entry.id === item) ?? list.find((entry) => entry.package?.id === item);
}

/** Новый текст персонажа из пакета, если шлюз его прислал; иначе — текст его карточки (persona.presets). */
function updateText(active: ActiveCharacter | null, persona: PersonaInfo | null, id: string): string | null {
    const update = active?.update;
    if (typeof update === 'string') return update;
    if (update && typeof update === 'object' && update.text) return update.text;
    return persona?.presets.find((preset) => preset.id === id)?.text ?? null;
}

/** Метки плитки: говорит, нужна модель, качается, не пойдёт здесь, новая версия, не из каталога, установлен. */
function characterPills(character: CharacterView, active: ActiveCharacter | null, entry?: CatalogEntry): Pill[] {
    const pills: Pill[] = [];
    const speaking = active?.id === character.id;
    if (speaking) pills.push({ text: tr.characters.speaks, cls: 'pill-own' });
    if (entry?.state === 'update' || (speaking && active?.update)) pills.push({ text: tr.characters.newVersion, cls: 'pill-agent' });
    if (character.problem) pills.push({ text: tr.characters.pillDoesntFit, cls: 'pill-off' });
    const voice = voicePill(character.voice);
    if (voice) pills.push(voice);
    // Чей голос — никто не проверял (свой источник): сказать прежде всего прочего о пакете.
    if (character.voiceOrigin === 'unknown') pills.push({ text: tr.characters.voiceNotChecked, cls: 'pill-yandex' });
    else if (character.package && !character.package.verified) pills.push({ text: tr.characters.notFromCatalog, cls: 'pill-off' });
    else if (character.package && pills.length === 0) pills.push({ text: tr.characters.pillInstalled, cls: 'pill-off' });
    return pills.slice(0, 2);
}

/** Плитка галереи: аватар, имя, строка, метки; ▶ — фраза-пробник, слышно до установки и без модели. */
function Tile({
    id,
    title,
    line,
    avatar,
    color,
    mark,
    sample,
    pills,
    active = false,
    dim = false,
    add = false,
    onOpen,
}: {
    id: string;
    title: string;
    line: string;
    avatar?: string | null;
    color?: string | null;
    mark?: 'voice';
    sample?: string | null;
    pills: Pill[];
    active?: boolean;
    dim?: boolean;
    add?: boolean;
    onOpen: () => void;
}) {
    return (
        <div class={`char-tile${active ? ' is-active' : ''}${dim ? ' is-dim' : ''}${add ? ' is-add' : ''}`}>
            <button type="button" class="char-open" onClick={onOpen}>
                {add ? (
                    <span class="char-avatar char-avatar-add" aria-hidden="true">
                        <IconPlus size={22} />
                    </span>
                ) : (
                    <Avatar src={avatar} title={title} color={color} mark={mark} />
                )}
                <span class="char-name">{title}</span>
                <span class="char-line">{line}</span>
                {pills.length > 0 && (
                    <span class="char-pills">
                        {pills.map((pill) => (
                            <span key={pill.text} class={`pill ${pill.cls}`}>
                                {pill.text}
                            </span>
                        ))}
                    </span>
                )}
            </button>
            <PlayButton id={`tile:${id}`} url={sample} label={title} />
        </div>
    );
}

export function CharactersTab() {
    const panel = usePanel();
    const linked = useLinkedItem('character', 'characters');
    // У компьютера-помощника персонажи дома ставятся у центра (п. 49, как MagicPlace и первые шаги): его каталог,
    // его картинки, его мастер установки — персонаж, слово, модель голоса, права.
    const station = panel.role.role === 'helper' ? panel.role.station?.key : undefined;
    const catalog = useCatalog(station);
    const media = installApi(station).mediaPath;
    const engines = useEngines();
    const [view, setView] = useState<CharactersView | null>(null);
    const [absent, setAbsent] = useState(false);
    const [error, setError] = useState<string | null>(null);
    const [persona, setPersona] = useState<PersonaInfo | null>(null);
    /** Ссылка вела с `?install`: после установки пакета открыть персонажа и окно «нужна модель». */
    const [intent, setIntent] = useState<string | null>(null);
    const mayEdit = can('persona.edit');

    const read = async (): Promise<void> => {
        try {
            setView(await api.characters());
            setError(null);
        } catch (e) {
            if (e instanceof ApiError && (e.status === 404 || e.status === 501)) setAbsent(true);
            else setError(problemOf(e));
        }
    };
    const readPersona = (): Promise<void> =>
        api
            .persona()
            .then(setPersona)
            .catch(() => undefined);
    const changed = (): void => {
        void read();
        void readPersona();
        void catalog.reload();
    };
    const actions = usePackageActions(changed, station);

    useEffect(() => {
        void read();
        void readPersona();
    }, [panel.version]);

    // Пока голос персонажа ставится — чаще: метка «качается N %» и строка голоса идут за установкой.
    const installing = !!view?.characters.some((entry) => entry.voice.state === 'installing');
    useEffect(() => {
        if (!installing) return;
        const id = setInterval(() => void read(), 2000);
        return () => clearInterval(id);
    }, [installing]);

    // `?install` — разовое: запомнили намерение, из адреса убрали (обновление страницы не откроет окно снова).
    const wantsInstall = 'install' in linked.query;
    useEffect(() => {
        if (!linked.item || !wantsInstall) return;
        setIntent(linked.item);
        linked.settle();
    }, [linked.item, wantsInstall]);

    if (absent) return <PersonaTab isAdmin={mayEdit} />;
    if (!view)
        return (
            <section class="card glass">
                <p class="t-meta m-0" role={error ? 'alert' : undefined}>
                    {error ?? tr.characters.reading}
                </p>
            </section>
        );

    const active = view.active;
    const entries = (catalog.view?.packages ?? []).filter((entry) => entry.kind === 'character');
    const entryOf = (character: CharacterView) => (character.package ? entries.find((entry) => entry.id === character.package!.id) : undefined);
    // Скрытые лентой (`unlisted`) — только со скрытой опцией разработчика, как в MagicPlace и первых шагах (п. 51);
    // `hidden` шлюз не присылает вовсе, поставленный — среди своих персонажей выше.
    const offered = catalogOpen(catalog.view)
        ? entries.filter((entry) => !isInstalled(entry) && (!entry.unlisted || !!catalog.view?.showHidden))
        : [];
    const status = catalogStatus(catalog);
    const ownSpeaks = !!active && active.id === null;

    const item = linked.item;
    const opened = item && item !== OWN ? findCharacter(view.characters, item) : undefined;
    const openedEntry = item && item !== OWN && !opened ? entries.find((entry) => entry.id === item) : undefined;
    const lost = !!item && item !== OWN && !opened && !openedEntry && (!!catalog.view || catalog.absent || !!catalog.error);

    const ownTile = (
        <Tile
            key={OWN}
            id={OWN}
            title={tr.characters.buildOwn}
            line={tr.characters.buildOwnLine}
            pills={ownSpeaks ? [{ text: tr.characters.speaks, cls: 'pill-own' }] : []}
            active={ownSpeaks}
            add
            onOpen={() => linked.open(OWN)}
        />
    );

    return (
        <>
            <section class="card glass card-list" aria-labelledby="characters-h">
                <div class="card-head">
                    <h2 id="characters-h" class="t-title-xs m-0">
                        {tr.characters.characters}
                    </h2>
                    <span class="t-meta">{tr.characters.fromNextQuestion}</span>
                </div>
                {active?.replacedBy && !view.characters.some((character) => character.package?.id === active.replacedBy) && (
                    <p class="t-caption m-0" role="status">
                        {tr.characters.majordomoReplaced}
                        <button class="underline cursor-pointer min-h-44" onClick={() => linked.open(active.replacedBy!)}>
                            {tr.characters.installArthur}
                        </button>
                        .
                    </p>
                )}
                <div class="char-grid">
                    {view.characters.map((character) => (
                        <Tile
                            key={character.id}
                            id={character.id}
                            title={character.title}
                            line={character.description}
                            avatar={character.avatar}
                            color={character.color}
                            mark={markOf(character)}
                            sample={character.samples[0]?.url}
                            pills={characterPills(character, active, entryOf(character))}
                            active={active?.id === character.id}
                            dim={!!character.problem}
                            onOpen={() => linked.open(character.id)}
                        />
                    ))}
                    {offered.length === 0 && ownTile}
                </div>

                {(offered.length > 0 || status || catalog.view) && (
                    <>
                        <div class="char-grid-head">
                            {/* Каталог — это MagicPlace (docs/plans/magicplace.md): заголовок ведёт туда, на персонажей. */}
                            <h3 class="t-title-xs m-0">
                                <button class="mp-link" onClick={() => panel.go('magicplace', 'store', undefined, { kind: 'character' })}>
                                    {tr.magicplace.charactersLink}
                                    <IconChevronRight size={14} />
                                </button>
                            </h3>
                            {catalog.view?.configured && catalog.view.enabled && (
                                <button class="t-caption cursor-pointer underline min-h-44" disabled={catalog.refreshing} onClick={() => void catalog.reload(true)}>
                                    {catalog.refreshing ? tr.characters.checking : tr.characters.checkNew}
                                </button>
                            )}
                        </div>
                        {status && (
                            <p class={`t-caption m-0 ${status.tone === 'warning' ? 'c-pill-yandex-fg' : ''}`} role="status">
                                {status.text}
                            </p>
                        )}
                        {feedLines(catalog.view).map((line) => (
                            <p key={line} class="t-caption m-0 c-pill-yandex-fg" role="status">
                                {line}
                            </p>
                        ))}
                        {offered.length > 0 && (
                            <div class="char-grid">
                                {offered.map((entry) => (
                                    <Tile
                                        key={`catalog:${entry.id}`}
                                        id={`catalog:${entry.id}`}
                                        title={entry.title}
                                        line={entry.description ?? ''}
                                        avatar={entry.preview?.image ? media(entry.preview.image) : null}
                                        sample={entry.preview?.samples?.[0] ? media(entry.preview.samples[0]) : undefined}
                                        pills={entryPills(entry, engines).slice(0, 2)}
                                        dim={isBlocked(entry)}
                                        onOpen={() => linked.open(entry.id)}
                                    />
                                ))}
                                {ownTile}
                            </div>
                        )}
                    </>
                )}
                {!mayEdit && <p class="t-caption m-0 mt-10">{tr.characters.onlyEditors}</p>}
            </section>

            {opened && (
                <CharacterSheet
                    key={opened.id}
                    character={opened}
                    active={active}
                    persona={persona}
                    entry={entryOf(opened)}
                    all={catalog.view?.packages}
                    actions={actions}
                    askNeeds={intent !== null && (intent === opened.id || intent === opened.package?.id)}
                    onAsked={() => setIntent(null)}
                    onPersona={setPersona}
                    onChanged={changed}
                    onClose={linked.close}
                />
            )}
            {openedEntry && (
                <CatalogEntrySheet entry={openedEntry} actions={actions} engines={engines} all={catalog.view?.packages} onClose={linked.close}>
                    {intent === openedEntry.id && !isInstalled(openedEntry) && (
                        <p class="win-note">{tr.characters.afterInstall}</p>
                    )}
                </CatalogEntrySheet>
            )}
            {item === OWN && <OwnSheet active={active} persona={persona} onPersona={setPersona} onChanged={changed} onClose={linked.close} />}
            {lost && (
                <Sheet title={tr.characters.notFoundTitle} meta={item ? <span class="win-code">{item}</span> : undefined} onClose={linked.close}>
                    <p class="win-lead">
                        {catalogOpen(catalog.view)
                            ? tr.characters.notInstalledNorCatalog(item)
                            : tr.characters.notInstalledCatalogDown(item)}
                    </p>
                </Sheet>
            )}
        </>
    );
}

/** Строка голоса персонажа: свой готов, звучит похожим (запасной) и почему, ставится. */
function VoiceLine({ character, onInstall }: { character: CharacterView; onInstall?: () => void }) {
    const voice = character.voice;
    const fallback = voice.fallback?.title;
    const similar = fallback ? tr.characters.voiceFallbackSimilar(fallback) : tr.characters.voiceFallbackReserve;
    let text: string;
    switch (voice.state) {
        case 'ready':
            text = tr.characters.voiceReady;
            break;
        case 'needs':
            text = tr.characters.voiceNeeds(similar);
            break;
        case 'installing':
            text = tr.characters.voiceInstalling(similar);
            break;
        case 'unsupported':
            text = tr.characters.voiceUnsupported(voice.reason ?? '', similar);
            break;
        case 'fallback':
            text = tr.characters.voiceFallback(similar, voice.reason ?? '');
            break;
        default:
            text = tr.characters.voiceNone;
    }
    return (
        <WinSection title={tr.characters.voice} group>
            <div class="win-row is-col">
                <div class="flex flex-wrap items-center gap-12">
                    <span class="win-row-s flex-1 min-w-0">{text}</span>
                    {voice.state === 'needs' && onInstall && (
                        <button class="btn btn-plain btn-sm" onClick={onInstall}>
                            {tr.characters.installReal}
                        </button>
                    )}
                </div>
                {voice.state === 'installing' && voice.progress !== undefined && <Progress percent={voice.progress} label={tr.characters.installingModel} />}
            </div>
        </WinSection>
    );
}

/** Карточка персонажа: аватар и приветствие, тоны, голос, «Выбрать», обновление, «Тонкая настройка», «Удалить». */
function CharacterSheet({
    character,
    active,
    persona,
    entry,
    all,
    actions,
    askNeeds,
    onAsked,
    onPersona,
    onChanged,
    onClose,
}: {
    character: CharacterView;
    active: ActiveCharacter | null;
    persona: PersonaInfo | null;
    entry?: CatalogEntry;
    /** Весь каталог: строка «В паре» (слово персонажа) находит в нём пакеты-пару. */
    all?: CatalogEntry[];
    actions: PackageActions;
    /** Пришли по ссылке «поставить»: голосу нужна модель — сразу окно. */
    askNeeds: boolean;
    onAsked: () => void;
    onPersona: (persona: PersonaInfo) => void;
    onChanged: () => void;
    onClose: () => void;
}) {
    const speaking = active?.id === character.id;
    const mayEdit = can('persona.edit');
    const mayVoice = can('models.manage');
    const initialTones = (): Record<string, number> =>
        Object.fromEntries(character.tones.map((tone) => [tone.id, (speaking ? active?.tones?.[tone.id] : undefined) ?? tone.default]));
    const [tones, setTones] = useState<Record<string, number>>(initialTones);
    const [busy, setBusy] = useState<'choose' | 'tones' | 'update' | 'text' | null>(null);
    const [note, setNote] = useState<CardNote>({});
    /** Памяти впритык (428): текст шлюза, следующее «Выбрать» — всё равно. */
    const [tight, setTight] = useState<string | null>(null);
    const [tuning, setTuning] = useState(false);
    const [draft, setDraft] = useState('');
    const editor = useRef<HTMLTextAreaElement | null>(null);
    const needs = useNeedsFlow((message) => {
        setNote({ notice: message });
        onChanged();
    });

    const tonesChanged = speaking && character.tones.some((tone) => (active?.tones?.[tone.id] ?? tone.default) !== tones[tone.id]);

    /** Окно «нужна модель»: «Скачать и включить» — выбор с install; запасной — выбор без модели (если ещё не выбран). */
    const askModel = (chosen: boolean): void => {
        const wanted = character.voice.needs;
        if (!wanted) return;
        needs.open({
            who: character.title,
            homeVoice: { character: character.id },
            needs: wanted,
            fallback: character.voice.fallback?.title,
            retry: ({ install, anyway }) => api.chooseCharacter(character.id, { tones, install, anyway }),
            onFallback: chosen ? undefined : () => api.chooseCharacter(character.id, { tones }),
        });
    };

    // По ссылке с `?install`: голосу нужна модель — окно сразу; не нужна — карточка с «Выбрать».
    useEffect(() => {
        if (!askNeeds) return;
        onAsked();
        if (character.voice.state === 'needs' && mayEdit && mayVoice) askModel(speaking);
    }, [askNeeds]);

    const choose = async (what: 'choose' | 'tones' | 'update', voice = true): Promise<void> => {
        if (!mayEdit) return;
        setBusy(what);
        setNote({});
        try {
            const result = await api.chooseCharacter(character.id, { tones, ...(tight !== null ? { anyway: true } : {}), ...(voice ? {} : { voice: false }) });
            setTight(null);
            const state = result.voice?.state;
            const wanted = result.voice?.needs ?? character.voice.needs;
            if (voice && state === 'needs' && wanted && !mayVoice)
                setNote({ notice: tr.characters.spokenReserveRestricted });
            else if (voice && state === 'needs' && wanted) {
                setNote({ notice: tr.characters.spokenReserveNeeds });
                needs.open({
                    who: character.title,
                    homeVoice: { character: character.id },
                    needs: wanted,
                    fallback: character.voice.fallback?.title,
                    retry: ({ install, anyway }) => api.chooseCharacter(character.id, { tones, install, anyway }),
                });
            } else if (what === 'update') setNote({ notice: tr.characters.updated });
            else if (what === 'tones') setNote({ notice: tr.characters.toneApplied });
            else setNote({ notice: result.voice?.pending ? tr.characters.chosenModelInstalling : tr.characters.chosen });
            onChanged();
        } catch (e) {
            const wanted = needsOf(e);
            if (isTight(e)) setTight(problemOf(e));
            else if (wanted && mayVoice)
                needs.open({
                    who: character.title,
                    homeVoice: { character: character.id },
                    needs: wanted,
                    fallback: character.voice.fallback?.title,
                    retry: ({ install, anyway }) => api.chooseCharacter(character.id, { tones, install, anyway }),
                });
            else setNote({ error: problemOf(e) });
        } finally {
            setBusy(null);
        }
    };

    const saveText = async (): Promise<void> => {
        setBusy('text');
        setNote({});
        try {
            // Непустой текст — правка этого персонажа (он остаётся выбранным); пустой — назад к стандартной.
            onPersona(draft.trim() ? await api.setCharacterText(character.id, draft.trim()) : await api.setPersonaText(''));
            setNote({ notice: draft.trim() ? tr.characters.savedSpeaks(character.title) : tr.characters.textCleared });
            onChanged();
        } catch (e) {
            setNote({ error: problemOf(e) });
        } finally {
            setBusy(null);
        }
    };

    const openTuning = (): void => {
        setTuning((open) => !open);
        setDraft(speaking ? (persona?.persona ?? '') : (persona?.presets.find((preset) => preset.id === character.id)?.text ?? ''));
    };

    // Окно модели — вместо карточки, не поверх: Esc и «мимо» закрывают одно окно, а не оба.
    if (needs.dialog) return needs.dialog;

    const greeting = character.samples[0];
    const fresh = speaking && active?.update ? updateText(active, persona, character.id) : null;
    const diff = fresh && persona ? diffText(persona.persona, fresh) : null;
    const removing = actions.busy[character.package?.id ?? ''];
    const removable = !!character.package && canManage('character');

    // Главная внизу: говорит — «Применить тон», когда тон поменяли; не говорит — «Выбрать».
    const footer = speaking ? (
        tonesChanged && mayEdit ? (
            <button class="btn btn-main" disabled={busy === 'tones'} aria-busy={busy === 'tones' || undefined} onClick={() => void choose('tones', false)}>
                {spin(busy === 'tones')}
                {tr.characters.applyTone}
            </button>
        ) : undefined
    ) : mayEdit ? (
        <button
            class="btn btn-main"
            disabled={busy === 'choose' || !!character.problem}
            aria-busy={busy === 'choose' || undefined}
            onClick={() => void choose('choose')}
        >
            {spin(busy === 'choose')}
            {tight !== null ? tr.characters.chooseAnyway : tr.characters.choose}
        </button>
    ) : undefined;

    return (
        <Sheet
            title={character.title}
            icon={<Avatar src={character.avatar} title={character.title} color={character.color} mark={markOf(character)} />}
            meta={
                <>
                    {speaking && <span class="pill pill-own">{tr.characters.speaks}</span>}
                    {character.voiceOrigin === 'unknown' && <span class="pill pill-yandex">{tr.characters.voiceNotChecked}</span>}
                    {character.package && !character.package.verified && <span class="pill pill-off">{tr.characters.notFromCatalog}</span>}
                    {character.builtin && <span class="win-chip">{tr.characters.builtin}</span>}
                    {character.package && <span class="win-chip">{tr.characters.versionMeta(character.package.version)}</span>}
                    {character.package?.license && <span class="win-chip">{character.package.license}</span>}
                </>
            }
            onClose={onClose}
            footer={footer}
        >
            {character.problem && (
                <p class="win-callout m-0" role="alert">
                    {character.problem}
                </p>
            )}
            {character.voiceOrigin === 'unknown' && <p class="win-callout m-0">{tr.characters.unknownVoiceOwner}</p>}
            {character.description && <p class="win-lead">{character.description}</p>}
            {greeting && (
                <div class="flex items-center gap-10">
                    <PlayButton id={`sheet:${character.id}`} url={greeting.url} label={character.title} className="char-play is-inline" />
                    <p class="char-greeting">{tr.characters.quoted(greeting.text)}</p>
                </div>
            )}
            {tight && (
                <p class="win-callout m-0" role="status">
                    {tight}
                </p>
            )}
            <NoteLine note={note} />
            <NoteLine note={character.package ? actions.notes[character.package.id] : undefined} />

            {character.tones.length > 0 && (
                <WinSection title={tr.characters.tone} group>
                    {character.tones.map((tone) => (
                        <div key={tone.id} class="win-row is-col">
                            <span class="win-row-t">{tone.title}</span>
                            <div role="radiogroup" aria-label={tone.title} class="tone-seg">
                                {tone.options.map((option, at) => (
                                    <button
                                        key={option.label}
                                        type="button"
                                        role="radio"
                                        class="seg-btn"
                                        aria-checked={tones[tone.id] === at}
                                        disabled={!mayEdit}
                                        onClick={() => setTones((all) => ({ ...all, [tone.id]: at }))}
                                    >
                                        {option.label}
                                    </button>
                                ))}
                            </div>
                        </div>
                    ))}
                </WinSection>
            )}

            <VoiceLine character={character} onInstall={mayEdit && mayVoice && character.voice.needs ? () => askModel(speaking) : undefined} />

            <Companions entry={entry} all={all} actions={actions} />

            {entry?.state === 'update' && (
                <WinSection title={tr.characters.inCatalogVersion(entry.version)} group>
                    <div class="win-row is-col">
                        <span class="win-row-s">{tr.characters.downloadsNew}</span>
                        {canManage('character') && (
                            <button
                                class="btn btn-plain btn-sm self-start"
                                disabled={!!actions.busy[entry.id]}
                                aria-busy={actions.busy[entry.id] === 'update' || undefined}
                                onClick={() => void actions.update(entry.id, entry.title)}
                            >
                                {spin(actions.busy[entry.id] === 'update')}
                                {tr.characters.downloadNew}
                            </button>
                        )}
                    </div>
                </WinSection>
            )}

            {speaking && active?.update && (
                <WinSection title={tr.characters.newVersionOtherCharacter} group>
                    <div class="win-row is-col">
                        {diff ? (
                            <div class="diff" aria-label={tr.characters.whatChanges}>
                                {diff.map((part, at) => (
                                    <span key={at} class={`diff-${part.op}`}>
                                        {part.op === 'del' ? '− ' : part.op === 'add' ? '+ ' : ''}
                                        {part.text}
                                    </span>
                                ))}
                            </div>
                        ) : (
                            <span class="win-row-s">{tr.characters.readingChanges}</span>
                        )}
                        <span class="win-row-s">
                            {active.edited ? tr.characters.editedKept : ''}
                            {tr.characters.untilUpdate}
                        </span>
                        {mayEdit && (
                            <button class="btn btn-plain btn-sm self-start" disabled={busy === 'update'} aria-busy={busy === 'update' || undefined} onClick={() => void choose('update', false)}>
                                {spin(busy === 'update')}
                                {tr.characters.update}
                            </button>
                        )}
                    </div>
                </WinSection>
            )}

            <WinSection
                title={tr.characters.fineTuning}
                aside={
                    <button class="btn btn-quiet btn-sm" aria-expanded={tuning} disabled={!persona} onClick={openTuning}>
                        {tuning ? tr.characters.hideText : tr.characters.showText}
                    </button>
                }
            >
                {tuning &&
                    (speaking && mayEdit ? (
                        <>
                            {active?.edited && <span class="win-hint">{tr.characters.editedByHand}</span>}
                            <PersonaEditor
                                editorRef={editor}
                                label={tr.characters.characterText(character.title)}
                                draft={draft}
                                onDraft={setDraft}
                                busy={busy === 'text'}
                                onSave={() => void saveText()}
                            />
                        </>
                    ) : draft ? (
                        <>
                            <pre class="prompt-view">{draft}</pre>
                            <span class="win-hint">{mayEdit ? tr.characters.editOnlySpeaking : tr.characters.adminChangesCharacter}</span>
                        </>
                    ) : (
                        <span class="win-hint">{tr.characters.fullTextLater}</span>
                    ))}
            </WinSection>

            {removable && (
                <WinSection title={tr.ui.danger} group>
                    <WinRow title={tr.characters.remove}>
                        <button
                            class="btn btn-danger btn-sm"
                            disabled={!!removing}
                            aria-busy={removing === 'remove' || undefined}
                            onClick={() =>
                                void actions.remove(character.package!.id, character.title, 'character').then((done) => {
                                    if (done) onClose();
                                })
                            }
                        >
                            {spin(removing === 'remove')}
                            {tr.characters.remove}
                        </button>
                    </WinRow>
                </WinSection>
            )}
        </Sheet>
    );
}

/** «Собрать своего»: свой текст характера и свой голос из лаборатории (или записать новый). */
function OwnSheet({
    active,
    persona,
    onPersona,
    onChanged,
    onClose,
}: {
    active: ActiveCharacter | null;
    persona: PersonaInfo | null;
    onPersona: (persona: PersonaInfo) => void;
    onChanged: () => void;
    onClose: () => void;
}) {
    const mayEdit = can('persona.edit');
    const mayVoice = can('models.manage');
    const own = !!active && active.id === null;
    const [draft, setDraft] = useState(() => (own ? (persona?.persona ?? '') : ''));
    const [saving, setSaving] = useState(false);
    const [note, setNote] = useState<CardNote>({});
    const [models, setModels] = useState<ModelsView | null>(null);
    /** Голоса не прочитать: шлюз без своих голосов (на колонке, absent) или ошибка. */
    const [noVoices, setNoVoices] = useState<{ absent: boolean; text: string } | null>(null);
    const [voiceNotes, setVoiceNotes] = useState<Record<string, CardNote>>({});
    const [voiceBusy, setVoiceBusy] = useState<Record<string, boolean>>({});
    const [tightFor, setTightFor] = useState<string | null>(null);
    const [wizard, setWizard] = useState(false);

    const readModels = (): void =>
        void api
            .models()
            .then((fresh) => {
                setModels(fresh);
                setNoVoices(null);
            })
            .catch((e) => setNoVoices({ absent: e instanceof ApiError && e.status === 501, text: problemOf(e) }));
    useEffect(readModels, []);
    // Текст пришёл позже окна — подставить, пока в поле ничего не писали.
    useEffect(() => {
        if (own && persona && !draft) setDraft(persona.persona);
    }, [persona]);

    const needs = useNeedsFlow((message) => {
        setNote({ notice: message });
        readModels();
    });

    const save = async (): Promise<void> => {
        setSaving(true);
        setNote({});
        try {
            onPersona(await api.setPersonaText(draft.trim()));
            setNote({ notice: draft.trim() ? tr.characters.spokenYours : tr.characters.returnedToStandard });
            onChanged();
        } catch (e) {
            setNote({ error: problemOf(e) });
        } finally {
            setSaving(false);
        }
    };

    const activate = async (cardId: string): Promise<void> => {
        const card = models?.tts.find((entry) => entry.id === cardId);
        if (!card) return;
        if (card.state === 'needs' && card.needs) {
            needs.open({ who: card.title, needs: card.needs, ownId: card.own?.id, retry: ({ install, anyway }) => api.activateTts(cardId, undefined, anyway, install) });
            return;
        }
        setVoiceBusy((all) => ({ ...all, [cardId]: true }));
        setVoiceNotes((all) => ({ ...all, [cardId]: { notice: tr.characters.loadingVoiceModel } }));
        try {
            await api.activateTts(cardId, undefined, tightFor === cardId);
            setTightFor(null);
            setVoiceNotes((all) => ({ ...all, [cardId]: { notice: tr.characters.speaksCard(card.title) } }));
            readModels();
        } catch (e) {
            const wanted = needsOf(e);
            if (isTight(e)) {
                setTightFor(cardId);
                setVoiceNotes((all) => ({ ...all, [cardId]: { warning: tr.characters.voiceNotEnabled(problemOf(e)) } }));
            } else if (wanted) {
                setVoiceNotes((all) => ({ ...all, [cardId]: {} }));
                needs.open({ who: card.title, needs: wanted, ownId: card.own?.id, retry: ({ install, anyway }) => api.activateTts(cardId, undefined, anyway, install) });
            } else setVoiceNotes((all) => ({ ...all, [cardId]: { error: problemOf(e) } }));
        } finally {
            setVoiceBusy((all) => ({ ...all, [cardId]: false }));
        }
    };

    // Мастер голоса и окно модели — вместо этого окна: одно окно за раз.
    if (wizard)
        return (
            <VoiceWizard
                onClose={() => {
                    setWizard(false);
                    readModels();
                }}
            />
        );
    if (needs.dialog) return needs.dialog;

    const ownCards = models?.tts.filter((card) => card.group === 'own') ?? [];
    return (
        <Sheet
            title={tr.characters.buildOwn}
            about={tr.characters.buildOwnAbout}
            icon={
                <span class="char-avatar char-avatar-add" aria-hidden="true">
                    <IconPlus size={22} />
                </span>
            }
            meta={own ? <span class="pill pill-own">{tr.characters.speaks}</span> : undefined}
            onClose={onClose}
        >
            <WinSection title={tr.characters.manner}>
                {mayEdit ? (
                    <PersonaEditor
                        label={tr.characters.ownCharacter}
                        draft={draft}
                        onDraft={setDraft}
                        busy={saving}
                        onSave={() => void save()}
                        placeholder={tr.characters.ownPlaceholder}
                    />
                ) : (
                    <p class="win-callout is-calm m-0">{tr.characters.adminChangesCharacter}</p>
                )}
                <NoteLine note={note} />
            </WinSection>
            <WinSection title={tr.characters.voice}>
                {noVoices ? (
                    <p class={noVoices.absent ? 'win-callout is-calm m-0' : 'win-callout m-0'} role={noVoices.absent ? undefined : 'alert'}>
                        {noVoices.absent ? tr.characters.ownVoiceOnComputer(noVoices.text) : noVoices.text}
                    </p>
                ) : !models ? (
                    <SkeletonRows rows={2} label={tr.characters.readingVoices} />
                ) : (
                    <>
                        {ownCards.length > 0 && (
                            <OwnVoiceRows
                                cards={ownCards}
                                isAdmin={mayVoice}
                                notes={voiceNotes}
                                busy={voiceBusy}
                                onActivate={(cardId) => void activate(cardId)}
                                tightFor={tightFor}
                                onChanged={readModels}
                            />
                        )}
                        {(ownCards.length === 0 || mayVoice) && (
                            <div class="win-group">
                                {ownCards.length === 0 && (
                                    <div class="win-row">
                                        <span class="win-row-s">{byMode(tr.characters.noOwnVoicesStation, tr.characters.noOwnVoicesDevice)}</span>
                                    </div>
                                )}
                                {mayVoice && (
                                    <div class="win-row">
                                        <button class="btn btn-plain btn-sm" onClick={() => setWizard(true)}>
                                            <IconPlus size={14} />
                                            {tr.characters.recordVoice}
                                        </button>
                                    </div>
                                )}
                            </div>
                        )}
                    </>
                )}
            </WinSection>
        </Sheet>
    );
}

/** «Сейчас отвечает: Артур» на главном экране — встроенный вклад в home.cards; открывает карточку персонажа. */
export function NowAnsweringCard() {
    const panel = usePanel();
    const [view, setView] = useState<CharactersView | null>(null);
    useEffect(() => {
        let alive = true;
        void api
            .characters()
            .then((fresh) => alive && setView(fresh))
            .catch(() => alive && setView(null));
        return () => {
            alive = false;
        };
    }, [panel.version]);
    const active = view?.active;
    if (!view || !active) return null;
    const character = active.id ? view.characters.find((entry) => entry.id === active.id) : undefined;
    const name = character?.title ?? (active.id ? active.id : tr.characters.ownLowerCase);
    const voice = character ? voicePill(character.voice) : null;
    return (
        <section class="card glass" aria-label={tr.characters.whoAnswers}>
            <button type="button" class="now-char" onClick={() => panel.go('character', 'characters', character?.id ?? OWN)}>
                <Avatar src={character?.avatar} title={name} color={character?.color} size="sm" mark={markOf(character)} />
                <span class="now-char-text">
                    <span class="t-meta">{tr.characters.nowAnswering}</span>
                    <span class="char-name">{name}</span>
                </span>
                {voice ? <span class={`pill ${voice.cls}`}>{voice.text}</span> : active.update ? <span class="pill pill-agent">{tr.characters.newVersion}</span> : null}
            </button>
        </section>
    );
}
