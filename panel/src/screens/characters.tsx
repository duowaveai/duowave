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
    can,
    isTight,
    needsOf,
    problemOf,
} from '../api';
import { IconPlus } from '../icons';
import { useLinkedItem, usePanel } from '../shell/context';
import { Action, CardNote } from '../ui/cards';
import { diffText } from '../ui/diff';
import { Avatar, PlayButton, markOf } from '../ui/media';
import { Progress, useNeedsFlow, voicePill } from '../ui/needs';
import { Sheet } from '../ui/sheet';
import {
    CatalogEntrySheet,
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

/** id «Собрать своего» в адресе: `#/mind/characters/own`. */
const OWN = 'own';

type Pill = { text: string; cls: string };

/** Персонаж по id из адреса: свой id (`arthur:arthur`, `standard`) или id его пакета (`arthur` — так ведёт сайт). */
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
    if (speaking) pills.push({ text: 'говорит', cls: 'pill-own' });
    if (entry?.state === 'update' || (speaking && active?.update)) pills.push({ text: 'новая версия', cls: 'pill-agent' });
    if (character.problem) pills.push({ text: 'не пойдёт здесь', cls: 'pill-off' });
    const voice = voicePill(character.voice);
    if (voice) pills.push(voice);
    // Чей голос — никто не проверял (свой источник): сказать прежде всего прочего о пакете.
    if (character.voiceOrigin === 'unknown') pills.push({ text: 'голос не проверен', cls: 'pill-yandex' });
    else if (character.package && !character.package.verified) pills.push({ text: 'не из каталога', cls: 'pill-off' });
    else if (character.package && pills.length === 0) pills.push({ text: 'установлен', cls: 'pill-off' });
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
    const catalog = useCatalog();
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
    const actions = usePackageActions(changed);

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
                    {error ?? 'Читаем персонажей…'}
                </p>
            </section>
        );

    const active = view.active;
    const entries = (catalog.view?.packages ?? []).filter((entry) => entry.kind === 'character');
    const entryOf = (character: CharacterView) => (character.package ? entries.find((entry) => entry.id === character.package!.id) : undefined);
    const offered = catalogOpen(catalog.view) ? entries.filter((entry) => !isInstalled(entry)) : [];
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
            title="Собрать своего"
            line="Свой голос из лаборатории и свой текст характера."
            pills={ownSpeaks ? [{ text: 'говорит', cls: 'pill-own' }] : []}
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
                        Персонажи
                    </h2>
                    <span class="t-meta">со следующего вопроса</span>
                </div>
                {active?.replacedBy && !view.characters.some((character) => character.package?.id === active.replacedBy) && (
                    <p class="t-caption m-0" role="status">
                        Мажордома теперь заменяет персонаж из каталога. Пока он говорит прежним текстом —{' '}
                        <button class="underline cursor-pointer min-h-44" onClick={() => linked.open(active.replacedBy!)}>
                            поставить Артура
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

                {(offered.length > 0 || status) && (
                    <>
                        <div class="char-grid-head">
                            <h3 class="t-title-xs m-0">Каталог</h3>
                            {catalog.view?.configured && catalog.view.enabled && (
                                <button class="t-caption cursor-pointer underline min-h-44" disabled={catalog.refreshing} onClick={() => void catalog.reload(true)}>
                                    {catalog.refreshing ? 'проверяю…' : 'проверить новые'}
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
                                        avatar={entry.preview?.image}
                                        sample={entry.preview?.samples?.[0]}
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
                {!mayEdit && <p class="t-caption m-0 mt-10">Выбирать и ставить персонажей может тот, кому можно менять характер.</p>}
            </section>

            {opened && (
                <CharacterSheet
                    key={opened.id}
                    character={opened}
                    active={active}
                    persona={persona}
                    entry={entryOf(opened)}
                    actions={actions}
                    askNeeds={intent !== null && (intent === opened.id || intent === opened.package?.id)}
                    onAsked={() => setIntent(null)}
                    onPersona={setPersona}
                    onChanged={changed}
                    onClose={linked.close}
                />
            )}
            {openedEntry && (
                <CatalogEntrySheet entry={openedEntry} actions={actions} engines={engines} onClose={linked.close}>
                    {intent === openedEntry.id && !isInstalled(openedEntry) && (
                        <p class="t-caption m-0">После установки откроется персонаж: выбрать его и, если нужно, поставить модель голоса.</p>
                    )}
                </CatalogEntrySheet>
            )}
            {item === OWN && <OwnSheet active={active} persona={persona} onPersona={setPersona} onChanged={changed} onClose={linked.close} />}
            {lost && (
                <Sheet title="Персонаж не найден" about={item ?? undefined} onClose={linked.close}>
                    <p class="t-body-sm m-0">
                        {catalogOpen(catalog.view)
                            ? `«${item}» нет ни среди поставленных, ни в каталоге.`
                            : `«${item}» здесь не поставлен, а каталог сейчас недоступен — попробуйте позже.`}
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
    const similar = fallback ? `Сейчас звучит похожим (${fallback})` : 'Сейчас звучит запасной голос';
    let text: string;
    switch (voice.state) {
        case 'ready':
            text = 'Голос свой — готов.';
            break;
        case 'needs':
            text = `${similar}.`;
            break;
        case 'installing':
            text = `Свой голос ставится. ${similar}.`;
            break;
        case 'unsupported':
            text = `Свой голос здесь не пойдёт${voice.reason ? `: ${voice.reason}` : ''}. ${similar}.`;
            break;
        case 'fallback':
            text = `${similar}${voice.reason ? ` — ${voice.reason}` : ''}.`;
            break;
        default:
            text = 'Своего голоса у персонажа нет — говорит голос, выбранный в «Голосе».';
    }
    return (
        <div class="char-block">
            <span class="t-meta fw-700">Голос</span>
            <div class="flex flex-wrap items-center gap-8">
                <span class="t-body-sm flex-1 min-w-0">{text}</span>
                {voice.state === 'needs' && onInstall && <Action tone="on" label="Поставить настоящий" onClick={onInstall} />}
            </div>
            {voice.state === 'installing' && voice.progress !== undefined && <Progress percent={voice.progress} label="Установка модели голоса" />}
        </div>
    );
}

/** Карточка персонажа: аватар и приветствие, тоны, голос, «Выбрать», обновление, «Тонкая настройка», «Удалить». */
function CharacterSheet({
    character,
    active,
    persona,
    entry,
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
                setNote({ notice: 'Говорит этим характером. Голос пока запасной: модель голоса ставит тот, кому можно менять модели.' });
            else if (voice && state === 'needs' && wanted) {
                setNote({ notice: 'Говорит этим характером. Голос пока запасной — нужна модель.' });
                needs.open({
                    who: character.title,
                    needs: wanted,
                    fallback: character.voice.fallback?.title,
                    retry: ({ install, anyway }) => api.chooseCharacter(character.id, { tones, install, anyway }),
                });
            } else if (what === 'update') setNote({ notice: 'Обновлён — со следующего вопроса.' });
            else if (what === 'tones') setNote({ notice: 'Тон применён — со следующего вопроса.' });
            else setNote({ notice: result.voice?.pending ? 'Выбран. Модель ставится — голос включится сам.' : 'Выбран — со следующего вопроса.' });
            onChanged();
        } catch (e) {
            const wanted = needsOf(e);
            if (isTight(e)) setTight(problemOf(e));
            else if (wanted && mayVoice)
                needs.open({
                    who: character.title,
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
            setNote({ notice: draft.trim() ? `Сохранено — ${character.title} говорит вашим текстом.` : 'Текст очищен — вернулись к стандартной.' });
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
    const meta = [character.package && `версия ${character.package.version}`, character.package?.license].filter(Boolean).join(' · ');
    const fresh = speaking && active?.update ? updateText(active, persona, character.id) : null;
    const diff = fresh && persona ? diffText(persona.persona, fresh) : null;
    const removing = actions.busy[character.package?.id ?? ''];

    return (
        <Sheet title={character.title} about={character.description} onClose={onClose}>
            <div class="char-hero">
                <Avatar src={character.avatar} title={character.title} color={character.color} size="lg" mark={markOf(character)} />
                <div class="char-hero-text">
                    {greeting && <p class="char-greeting">«{greeting.text}»</p>}
                    <div class="flex flex-wrap items-center gap-8">
                        {greeting && <PlayButton id={`sheet:${character.id}`} url={greeting.url} label={character.title} className="char-play is-inline" />}
                        {speaking && <span class="pill pill-own">говорит</span>}
                        {character.package && !character.package.verified && <span class="pill pill-off">не из каталога</span>}
                        {character.voiceOrigin === 'unknown' && <span class="pill pill-yandex">голос не проверен</span>}
                        {character.builtin && <span class="pill pill-off">встроенный</span>}
                    </div>
                    {meta && <span class="t-caption">{meta}</span>}
                    {character.voiceOrigin === 'unknown' && (
                        <span class="t-caption c-pill-yandex-fg">Чей это голос и есть ли согласие его хозяина, никто не проверял: персонаж из своего источника.</span>
                    )}
                </div>
            </div>
            {character.problem && (
                <p class="t-caption m-0 c-dot-offline" role="alert">
                    {character.problem}
                </p>
            )}

            {character.tones.length > 0 && (
                <div class="char-block">
                    <span class="t-meta fw-700">Тон</span>
                    {character.tones.map((tone) => (
                        <div key={tone.id} class="tone-row">
                            <span class="t-caption">{tone.title}</span>
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
                </div>
            )}

            <VoiceLine character={character} onInstall={mayEdit && mayVoice && character.voice.needs ? () => askModel(speaking) : undefined} />

            <div class="char-actions">
                {speaking ? (
                    tonesChanged && mayEdit && <Action tone="on" label="Применить тон" busy={busy === 'tones'} onClick={() => void choose('tones', false)} />
                ) : (
                    mayEdit && (
                        <Action
                            tone="on"
                            label={tight !== null ? 'Выбрать всё равно' : 'Выбрать'}
                            busy={busy === 'choose'}
                            disabled={!!character.problem}
                            onClick={() => void choose('choose')}
                        />
                    )
                )}
                <Action label={tuning ? 'Скрыть текст' : 'Тонкая настройка'} disabled={!persona} onClick={openTuning} />
                {character.package && canManage('character') && (
                    <Action
                        label="Удалить"
                        busy={removing === 'remove'}
                        disabled={!!removing}
                        onClick={() =>
                            void actions.remove(character.package!.id, character.title, 'character').then((done) => {
                                if (done) onClose();
                            })
                        }
                    />
                )}
            </div>
            {tight && (
                <p class="t-caption m-0 c-pill-yandex-fg" role="status">
                    {tight}
                </p>
            )}
            <NoteLine note={note} />
            <NoteLine note={character.package ? actions.notes[character.package.id] : undefined} />

            {entry?.state === 'update' && (
                <div class="char-block">
                    <span class="t-meta fw-700">В каталоге версия {entry.version}</span>
                    <span class="t-caption">
                        Скачается новая версия персонажа. Что говорит персонаж, поменяется только после «Обновить» — сначала покажем, что изменится.
                    </span>
                    {canManage('character') && (
                        <div>
                            <Action
                                tone="on"
                                label="Скачать новую версию"
                                busy={actions.busy[entry.id] === 'update'}
                                disabled={!!actions.busy[entry.id]}
                                onClick={() => void actions.update(entry.id, entry.title)}
                            />
                        </div>
                    )}
                </div>
            )}

            {speaking && active?.update && (
                <div class="char-block">
                    <span class="t-meta fw-700">В новой версии другой характер</span>
                    {diff ? (
                        <div class="diff" aria-label="Что поменяется в характере">
                            {diff.map((part, at) => (
                                <span key={at} class={`diff-${part.op}`}>
                                    {part.op === 'del' ? '− ' : part.op === 'add' ? '+ ' : ''}
                                    {part.text}
                                </span>
                            ))}
                        </div>
                    ) : (
                        <span class="t-caption">Читаем, что поменялось…</span>
                    )}
                    <span class="t-caption">
                        {active.edited ? 'Текст правили вручную: ваши правки останутся. ' : ''}Пока не нажмёте «Обновить», говорит прежним текстом.
                    </span>
                    {mayEdit && (
                        <div>
                            <Action tone="on" label="Обновить" busy={busy === 'update'} onClick={() => void choose('update', false)} />
                        </div>
                    )}
                </div>
            )}

            {tuning && (
                <div class="char-block">
                    <span class="t-meta fw-700">Тонкая настройка</span>
                    {speaking && mayEdit ? (
                        <>
                            {active?.edited && <span class="t-caption">Текст уже правили вручную.</span>}
                            <PersonaEditor
                                editorRef={editor}
                                label={`Текст характера: ${character.title}`}
                                draft={draft}
                                onDraft={setDraft}
                                busy={busy === 'text'}
                                onSave={() => void saveText()}
                            />
                        </>
                    ) : draft ? (
                        <>
                            <pre class="prompt-view">{draft}</pre>
                            <span class="t-caption">
                                {mayEdit ? 'Править текст можно у того, кто говорит: сначала «Выбрать».' : 'Менять характер может администратор.'}
                            </span>
                        </>
                    ) : (
                        <span class="t-caption">Полный текст появится, когда персонаж заговорит.</span>
                    )}
                </div>
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
            setNote({ notice: draft.trim() ? 'Говорит вашим характером.' : 'Вернулись к стандартной.' });
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
        setVoiceNotes((all) => ({ ...all, [cardId]: { notice: 'Загружаю модель голоса — до минуты…' } }));
        try {
            await api.activateTts(cardId, undefined, tightFor === cardId);
            setTightFor(null);
            setVoiceNotes((all) => ({ ...all, [cardId]: { notice: `Говорит «${card.title}».` } }));
            readModels();
        } catch (e) {
            const wanted = needsOf(e);
            if (isTight(e)) {
                setTightFor(cardId);
                setVoiceNotes((all) => ({ ...all, [cardId]: { warning: `Голос не включён. ${problemOf(e)}` } }));
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
        <Sheet title="Собрать своего" about="свой текст и свой голос" onClose={onClose}>
            <div class="char-block">
                <span class="t-meta fw-700">Манера</span>
                {own && <span class="pill pill-own self-start">говорит</span>}
                {mayEdit ? (
                    <PersonaEditor
                        label="Свой характер"
                        draft={draft}
                        onDraft={setDraft}
                        busy={saving}
                        onSave={() => void save()}
                        placeholder="Кто отвечает и как говорит: «Ты — старый морской волк, отвечаешь коротко и с юмором…» О себе — в женском или мужском роде."
                    />
                ) : (
                    <span class="t-caption">Менять характер может администратор.</span>
                )}
                <NoteLine note={note} />
            </div>
            <div class="char-block">
                <div class="flex items-center justify-between gap-8">
                    <span class="t-meta fw-700">Голос</span>
                    {mayVoice && !noVoices && models && (
                        <button class="pill pill-own pill-btn-sm inline-flex items-center gap-6" onClick={() => setWizard(true)}>
                            <IconPlus size={14} />
                            Записать голос
                        </button>
                    )}
                </div>
                {noVoices ? (
                    <span class={noVoices.absent ? 't-caption' : 't-caption c-dot-offline'} role={noVoices.absent ? undefined : 'alert'}>
                        {noVoices.absent ? `Свой голос — на компьютере. ${noVoices.text}` : noVoices.text}
                    </span>
                ) : !models ? (
                    <span class="t-caption">Читаем голоса…</span>
                ) : ownCards.length === 0 ? (
                    <span class="t-caption">Своих голосов пока нет. Прочитайте вслух минуту-две текста — и колонка будет отвечать этим голосом.</span>
                ) : (
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
            </div>
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
    const name = character?.title ?? (active.id ? active.id : 'свой характер');
    const voice = character ? voicePill(character.voice) : null;
    return (
        <section class="card glass" aria-label="Кто отвечает">
            <button type="button" class="now-char" onClick={() => panel.go('character', 'characters', character?.id ?? OWN)}>
                <Avatar src={character?.avatar} title={name} color={character?.color} size="sm" mark={markOf(character)} />
                <span class="now-char-text">
                    <span class="t-meta">Сейчас отвечает</span>
                    <span class="char-name">{name}</span>
                </span>
                {voice ? <span class={`pill ${voice.cls}`}>{voice.text}</span> : active.update ? <span class="pill pill-agent">новая версия</span> : null}
            </button>
        </section>
    );
}
