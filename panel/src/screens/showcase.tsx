// Витрина стола (docs/plans/start-desk.md; холст «Стартовый стол — лаконично», доски «Б · 1–3» и «Без колонки · 1–2»):
// под сценой три карточки — «скажи — и сделается». Сначала честно: нет ИИ — шаг 1 «Сделайте Алису умнее» (выбор ИИ той
// же логикой, что у первых шагов, screens/brain-connect.ts); ИИ есть, а колонка в «Как раньше» — «Включить «Вместе»».
// Змейка играет кнопкой и до того: ей ИИ не нужен. ИИ подключили здесь — «Вместе» включается сразу (владелец, 16.11),
// рядом «Отменить», и одна большая карточка «Теперь Алиса умнее» с тремя фразами. Фразу можно сказать или нажать —
// нажатие уходит в разговор, как сказанная (answerTo). Попробовали всё — витрина уходит сама. Встал на стол первый свой
// виджет — сделанный или змейка «Оставить на столе» — витрина тоже уходит (решение владельца 18.11, проверка начисто:
// своё на столе — витрина своё дело сделала, и стол не тесный): так же, как «Скрыть витрину», — как виджет, один раз;
// вернуть — из «+ Новый виджет». Что попробовали — в памяти пульта у каждого человека (ключ `showcase`).

import { useEffect, useRef, useState } from 'preact/hooks';

import { PanelMode, api, can, problemOf } from '../api';
import { tr } from '../i18n';
import { usePanel } from '../shell/context';
import { recalled, remember, useRecalled } from '../shell/memory';
import { useSlot } from '../shell/slot';
import { deskActions, useDeskLayout } from '../shell/widgets';
import { useWidgetEmpty } from '../ui/widget';
import { onComputer } from '../voice-input';
import { useDeskDrafts } from '../voice/drafts';
import { answerTo } from '../voice/speak';
import { PATHS, Path, useBrainConnect } from './brain-connect';
import { appSectionOf } from './plugins';
import { useCallWord } from './call-word';
import { installProgress, onInstallProgress } from '../magicplace/wizard-state';
import '../ui/showcase.css';

type Card = 'talk' | 'play' | 'make';

/** Память витрины у человека: что попробовал, что решил со змейкой, видел ли «Теперь умнее», что вернуть «Отменить». */
interface Memo {
    tried?: Card[];
    snake?: 'kept' | 'dropped';
    smarter?: 'show' | 'seen';
    /** Режим колонки до «Вместе», который включила витрина: его возвращает «Отменить». */
    undo?: PanelMode;
    /** Кем подключили ИИ — для «✓ OpenRouter подключён». */
    title?: string;
    /** Витрина ушла сама, когда на стол встал первый свой виджет: второй раз не уходит (её вернули из «+»). */
    left?: boolean;
}

const KEY = 'showcase';
/** Змейка — встроенное приложение; её плитку «Оставить на столе» ставит на стол. */
const SNAKE = 'neon-snake';
const SNAKE_TILE = `package.${SNAKE}.tile`;
const SHOWCASE = 'core.showcase';

const save = (patch: Partial<Memo>): void => remember(KEY, { ...(recalled<Memo>(KEY) ?? {}), ...patch });

/** Клетки змейки на доске карточки: столбец и строка сетки в 12 точек. */
const SNAKE_CELLS: Array<[number, number]> = [[3, 5], [4, 5], [5, 5], [6, 5], [7, 5], [7, 4], [7, 3], [8, 3], [9, 3]];
const FOOD: [number, number] = [15, 3];

const Wave = () => (
    <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" aria-hidden="true">
        <path d="M4 10v4M8 7v10M12 4v16M16 7v10M20 10v4" />
    </svg>
);
const Lock = () => (
    <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
        <rect x="5" y="11" width="14" height="10" rx="2" />
        <path d="M8 11V8a4 4 0 0 1 8 0v3" />
    </svg>
);
const Chat = () => (
    <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linejoin="round" aria-hidden="true">
        <path d="M4 5h16v11H8l-4 4z" />
    </svg>
);
const Game = () => (
    <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" aria-hidden="true">
        <rect x="3" y="7" width="18" height="10" rx="3" />
        <path d="M8 12h3M9.5 10.5v3M15 11.5h.01M17 12.5h.01" />
    </svg>
);
const Spark = () => (
    <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linejoin="round" aria-hidden="true">
        <path d="M12 3l1.8 5.2L19 10l-5.2 1.8L12 17l-1.8-5.2L5 10l5.2-1.8z" />
    </svg>
);

function SnakeArt() {
    return (
        <div class="sc-art sc-snake" aria-hidden="true">
            {SNAKE_CELLS.map(([x, y]) => (
                <i key={`${x}.${y}`} style={{ left: `${1 + x * 12}px`, top: `${1 + y * 12}px` }} />
            ))}
            <i class="is-food" style={{ left: `${1 + FOOD[0] * 12}px`, top: `${1 + FOOD[1] * 12}px` }} />
        </div>
    );
}

export function ShowcaseWidget() {
    const t = tr.showcase;
    const panel = usePanel();
    const computer = onComputer();
    const memo = useRecalled<Memo>(KEY) ?? {};
    const brain = useBrainConnect();
    /** Режим колонки: undefined — читаю, null — не узнал. На компьютере без колонки режима нет. */
    const [mode, setMode] = useState<PanelMode | null | undefined>(undefined);
    const [busy, setBusy] = useState(false);
    const [problem, setProblem] = useState<string | null>(null);
    const [name, setName] = useState<string | null>(null);
    /** Голос выбранного персонажа ещё ставится (модель качается): пока отвечает голос Алисы (п. 55). */
    const [voiceComing, setVoiceComing] = useState(false);
    const word = useCallWord();
    const [installing, setInstalling] = useState(installProgress);
    useEffect(() => onInstallProgress(() => setInstalling(installProgress())), []);
    const [at, setAt] = useState(0);
    // Приложения пульта приходят позже стола: перерисоваться, когда змейка появится или уйдёт.
    useSlot('desk.widgets');
    const snake = appSectionOf(SNAKE);
    const { tile: draft, landed } = useDeskDrafts();
    const [layout, actions] = useDeskLayout(panel.role.role);
    /** Свой виджет на столе: сделанный (сел из «Примерки» или черновик уже на столе) или оставленная змейка. */
    const own = landed || layout.added.some((key) => key.startsWith('package.'));
    const mayChoose = can('models.manage');

    useEffect(() => {
        if (computer) return;
        let alive = true;
        void api
            .mode()
            .then((view) => alive && setMode(view.mode))
            .catch(() => alive && setMode(null));
        return () => {
            alive = false;
        };
    }, [computer, panel.version]);

    useEffect(() => {
        let alive = true;
        void api
            .characters()
            .then((view) => {
                if (!alive) return;
                const id = view.active?.id;
                const active = id ? view.characters.find((entry) => entry.id === id) : undefined;
                setName(active?.title ?? null);
                setVoiceComing(!!active && !active.builtin && active.voice.state === 'installing');
            })
            .catch(() => undefined);
        return () => {
            alive = false;
        };
    }, [panel.version, installing?.done, installing?.finished]);

    const tried = new Set<Card>(memo.tried ?? []);
    const mark = (card: Card): void => {
        const before = recalled<Memo>(KEY) ?? {};
        save({ tried: [...new Set([...(before.tried ?? []), card])], smarter: before.smarter === 'show' ? 'seen' : before.smarter });
    };
    // Черновик на столе — «Сделайте своё» уже попробовали, сказали ли фразу голосом или нажали.
    useEffect(() => {
        if (draft && !tried.has('make')) mark('make');
    }, [!!draft]);

    const providerTitle = (): string | undefined => {
        const view = brain.view;
        const id = view?.choice.fast?.provider;
        return view?.providers.find((provider) => provider.id === id)?.title;
    };

    /** ИИ только что подключили здесь: колонка в «Как раньше» — сразу «Вместе», и «Теперь Алиса умнее». */
    const connected = async (): Promise<void> => {
        const patch: Partial<Memo> = { smarter: 'show', title: providerTitle() };
        if (!computer) {
            try {
                const view = await api.mode();
                if (view.mode === 'relay') {
                    setMode((await api.setMode('gateway')).mode);
                    patch.undo = 'relay';
                } else setMode(view.mode);
            } catch (e) {
                setProblem(problemOf(e));
            }
        }
        save(patch);
    };
    const was = useRef<boolean | null>(null);
    useEffect(() => {
        if (!brain.view) return;
        if (was.current === false && brain.answering) void connected();
        was.current = brain.answering;
    }, [brain.view, brain.answering]);

    const run = async (job: () => Promise<void>): Promise<void> => {
        setBusy(true);
        setProblem(null);
        try {
            await job();
        } catch (e) {
            setProblem(problemOf(e));
        } finally {
            setBusy(false);
        }
    };
    const together = () =>
        void run(async () => {
            setMode((await api.setMode('gateway')).mode);
            save({ smarter: 'show', undo: 'relay', title: providerTitle() });
        });
    const undo = () =>
        void run(async () => {
            if (!memo.undo) return;
            setMode((await api.setMode(memo.undo)).mode);
            save({ undo: undefined, smarter: 'seen' });
        });

    /** Фраза — в разговор, как сказанная; на компьютере без колонки ответ звучит его голосом. */
    const say = (text: string, card: Card): void => {
        answerTo({ text, via: 'text' }, undefined, computer);
        mark(card);
    };
    const play = (): void => {
        if (snake) panel.go(snake);
        mark('play');
    };
    const keep = (): void => {
        deskActions(panel.role.role).add(SNAKE_TILE);
        save({ snake: 'kept' });
    };
    const hide = (): void => deskActions(panel.role.role).hide(SHOWCASE);
    // Первый свой виджет на столе — витрина уходит, один раз.
    useEffect(() => {
        if (!own || recalled<Memo>(KEY)?.left) return;
        save({ left: true });
        actions.hide(SHOWCASE);
    }, [own]);

    const cards: Card[] = snake ? ['talk', 'play', 'make'] : ['talk', 'make'];
    const finished = cards.every((card) => tried.has(card)) && (!snake || !!memo.snake);
    const hasBrain = brain.answering;
    const relay = !computer && mode === 'relay';
    const phase = !hasBrain ? 'brain' : relay ? 'switch' : memo.smarter === 'show' ? 'smarter' : 'try';
    const empty = !brain.view || (!computer && mode === undefined) || finished;
    useWidgetEmpty(empty);
    if (empty) return null;

    const locked = phase === 'brain' || phase === 'switch';
    // Слово, которое на деле зовёт (п. 54): стоит приложение-слово — его; нет — прежнее.
    const talkMe = word ? t.talkWith(word) : computer ? t.talkMeComputer : t.talkMe;
    const makeMe = word ? t.makeWith(word) : computer ? t.makeMeComputer : t.makeMe;
    const playMe = word ? t.playWith(word) : computer ? t.playMeComputer : t.playMe;
    /** Имя того, кто ответит, без пометки «(демо)». */
    const who = name ? name.replace(/\s*\([^)]*\)\s*$/, '').trim() || name : null;
    const voiceNote = (voiceComing || (installing && !installing.finished && installing.kind === 'character')) && who;
    const failed = problem && (
        <p class="sc-problem" role="alert">
            {problem}
        </p>
    );

    if (phase === 'smarter')
        return (
            <section class="sc sc-card sc-smarter" aria-label={computer ? t.smarterComputer : t.smarterStation}>
                <div class="sc-smarter-top">
                    <h2>{computer ? t.smarterComputer : t.smarterStation}</h2>
                    {memo.title && <span class="sc-chip">{t.smarterConnected(memo.title)}</span>}
                    {memo.undo && <span class="sc-chip">{t.smarterTogether}</span>}
                    {memo.undo && (
                        <button type="button" class="sc-soft" aria-disabled={busy ? 'true' : undefined} onClick={() => !busy && undo()}>
                            {t.undo}
                        </button>
                    )}
                </div>
                <p class="sc-lead">{computer ? t.smarterLeadComputer : t.smarterLeadStation}</p>
                <span class="sc-label sc-say-and-see">{t.sayAndSee}</span>
                <div class="sc-tiles">
                    <button type="button" class="sc-tile" title={t.pressNote} onClick={() => say(t.talkSay, 'talk')}>
                        <span class="sc-tile-k">
                            <Chat />
                            {t.talk}
                        </span>
                        <span class="sc-tile-p">«{talkMe}»</span>
                    </button>
                    {snake && (
                        <button type="button" class="sc-tile" onClick={play}>
                            <span class="sc-tile-k">
                                <Game />
                                {t.play}
                            </span>
                            <span class="sc-tile-p">{t.playGame}</span>
                        </button>
                    )}
                    <button type="button" class="sc-tile" title={t.pressNote} onClick={() => say(t.makeSay, 'make')}>
                        <span class="sc-tile-k">
                            <Spark />
                            {t.make}
                        </span>
                        <span class="sc-tile-p">«{makeMe}»</span>
                    </button>
                </div>
                {failed}
            </section>
        );

    const done = cards.filter((card) => tried.has(card)).length;
    const head =
        phase === 'brain' ? (computer ? t.headComputer : t.headStation) : phase === 'switch' ? t.headSwitch : done ? t.headDone(done, cards.length) : t.headTry;

    const pathTitle: Record<Path, [string, string]> = {
        openrouter: [t.pathOr, t.pathOrNote],
        chatgpt: [t.pathGpt, t.pathGptNote],
        key: [t.pathKey, t.pathKeyNote],
        local: [t.pathLocal, t.pathLocalNote],
    };
    const checking = brain.checking?.path === brain.path ? brain.checking : null;
    const said = brain.problem?.path === brain.path ? brain.problem.text : null;
    const signInRow = (which: 'openrouter' | 'chatgpt', label: string, note: string) => (
        <>
            <div class="sc-acts">
                <button
                    type="button"
                    class="sc-btn"
                    aria-disabled={!brain.view || brain.signIn.starting ? 'true' : undefined}
                    onClick={() => brain.beginSignIn(which)}
                >
                    {brain.signIn.starting && brain.signIn.target === which ? t.opening : label}
                </button>
                <span class="sc-note">{note}</span>
            </div>
            {brain.signIn.target === which && brain.signIn.steps && <div class="sc-steps">{brain.signIn.steps}</div>}
        </>
    );
    const brainActions = !mayChoose ? (
        <span class="sc-note">{t.adminOnly}</span>
    ) : checking ? (
        <span class="sc-note" role="status">
            {t.checking(checking.title)}
        </span>
    ) : brain.path === 'openrouter' ? (
        signInRow('openrouter', t.signInOr, t.payOr)
    ) : brain.path === 'chatgpt' ? (
        signInRow('chatgpt', t.signInGpt, t.payGpt)
    ) : brain.path === 'key' ? (
        <form
            class="sc-acts"
            onSubmit={(e) => {
                e.preventDefault();
                if (brain.keyReady) void brain.saveKey();
            }}
        >
            <input
                class="sc-key"
                type="password"
                autocomplete="off"
                aria-label={t.keyLabel}
                placeholder={t.keyPlaceholder}
                value={brain.key}
                onInput={(e) => brain.setKey((e.target as HTMLInputElement).value)}
            />
            {brain.keyFields.map((field) => (
                <input
                    key={field.id}
                    class="sc-key"
                    autocomplete="off"
                    aria-label={field.title}
                    placeholder={field.placeholder ?? field.title}
                    value={brain.fields[field.id] ?? ''}
                    onInput={(e) => brain.setFields((all) => ({ ...all, [field.id]: (e.target as HTMLInputElement).value }))}
                />
            ))}
            <button type="submit" class="sc-btn" aria-disabled={!brain.keyReady ? 'true' : undefined}>
                {t.keyCheck}
            </button>
            <span class="sc-note sc-note-full">{brain.keyPreset ? t.keyKnown(brain.keyPreset.title) : t.keyNote}</span>
        </form>
    ) : brain.local === 'none' ? (
        <div class="sc-acts">
            <button type="button" class="sc-btn" onClick={() => void brain.findLocal(true)}>
                {tr.onbBrain.localAgain}
            </button>
            <span class="sc-note">{t.localNone}</span>
        </div>
    ) : (
        <span class="sc-note" role="status">
            {t.localSearching}
        </span>
    );

    const first =
        phase === 'brain' ? (
            <section class="sc-card is-wide" aria-label={computer ? t.stepComputer : t.stepStation}>
                <span class="sc-card-h is-strong">{computer ? t.stepComputer : t.stepStation}</span>
                <div class="sc-art is-list" role="radiogroup" aria-label={t.pathsLabel}>
                    {PATHS.map((path) => (
                        <label key={path} class={`sc-opt${brain.path === path ? ' is-on' : ''}`}>
                            <input type="radio" name="sc-brain" checked={brain.path === path} disabled={!mayChoose} onChange={() => brain.choosePath(path)} />
                            <span class="sc-opt-title">{pathTitle[path][0]}</span>
                            <span class="sc-opt-note">{pathTitle[path][1]}</span>
                        </label>
                    ))}
                </div>
                {brainActions}
                {said && (
                    <p class="sc-problem" role="alert">
                        {said}
                    </p>
                )}
            </section>
        ) : phase === 'switch' ? (
            <section class="sc-card is-wide" aria-label={t.switchTitle}>
                <span class="sc-card-h is-strong">{t.switchTitle}</span>
                <div class="sc-art is-list">
                    <div class="sc-cmp">
                        <span>{t.cmpHow}</span>
                        <span class="sc-who">{t.cmpHowWho(name ?? t.yourAi)}</span>
                    </div>
                    <div class="sc-cmp">
                        <span>{t.cmpWidget}</span>
                        <span class="sc-who">{t.cmpWidgetWho}</span>
                    </div>
                    <div class="sc-cmp">
                        <span>{t.cmpJazz}</span>
                        <span class="sc-who">{t.cmpJazzWho}</span>
                    </div>
                </div>
                <div class="sc-acts">
                    <button type="button" class="sc-btn" aria-disabled={busy || !can('settings.edit') ? 'true' : undefined} onClick={() => !busy && together()}>
                        {busy ? t.switching : t.switchOn}
                    </button>
                </div>
            </section>
        ) : (
            <section class="sc-card" aria-label={t.talk}>
                <span class="sc-card-h">
                    {t.talk}
                    {tried.has('talk') && <span class="sc-chip sc-chip-end">✓</span>}
                </span>
                <div class="sc-art sc-bubbles">
                    <span class="sc-bub is-me">{talkMe}</span>
                    <span class="sc-bub is-ai">
                        <b>{name ?? 'DuoWave'}:</b> {t.talkReply}
                    </span>
                </div>
                <button type="button" class="sc-say" title={t.pressNote} onClick={() => say(t.talkSay, 'talk')}>
                    <Wave />«{talkMe}»
                </button>
                {who && <span class="sc-who">{t.answers(who)}</span>}
                {voiceNote && <span class="sc-who">{t.voiceComing(who)}</span>}
            </section>
        );

    const playCard = snake && (
        <section class="sc-card" aria-label={t.play}>
            <span class="sc-card-h">
                {t.play}
                {tried.has('play') && <span class="sc-chip sc-chip-end">{memo.snake === 'kept' ? t.kept : t.played}</span>}
            </span>
            <SnakeArt />
            {!tried.has('play') ? (
                <div class="sc-acts">
                    <button type="button" class="sc-btn" onClick={play}>
                        {t.playBtn}
                    </button>
                    {locked && <span class="sc-note">{computer ? t.playNoteComputer : t.playNoteStation}</span>}
                </div>
            ) : !memo.snake ? (
                <div class="sc-acts">
                    <button type="button" class="sc-btn" onClick={keep}>
                        {t.keep}
                    </button>
                    <button type="button" class="sc-soft" onClick={() => save({ snake: 'dropped' })}>
                        {t.drop}
                    </button>
                </div>
            ) : null}
            {/* Змейка сделана голосом — и своя игра так же (замечание 4 теста 19.11). Фраза — в разговор, как сказанная;
                карточку «сыграли» она не отмечает: это просьба сделать новую, а не игра в змейку. */}
            {!locked && (
                <button
                    type="button"
                    class="sc-say"
                    title={t.pressNote}
                    onClick={() => answerTo({ text: t.playSay, via: 'text' }, undefined, computer)}
                >
                    <Wave />«{playMe}»
                </button>
            )}
            <span class="sc-who">{t.playMade}</span>
        </section>
    );

    const makeCard = (
        <section class="sc-card" aria-label={t.make}>
            <span class="sc-card-h">
                {t.make}
                {tried.has('make') && <span class="sc-chip sc-chip-end">{t.made}</span>}
            </span>
            <div class={`sc-art sc-make${locked ? ' is-dim' : ''}`}>
                {draft && !locked && <span class="sc-chip is-busy">{t.makeBusy}</span>}
                <span class="sc-bar" style={{ width: '46%' }} />
                <span class="sc-bar is-big" style={{ width: '70%' }} />
                <span class="sc-bar" style={{ width: '58%' }} />
            </div>
            {locked ? (
                <span class="sc-chip is-lock">
                    <Lock />
                    {t.makeLocked}
                </span>
            ) : (
                <button type="button" class="sc-say" title={t.pressNote} onClick={() => say(t.makeSay, 'make')}>
                    <Wave />«{makeMe}»
                </button>
            )}
        </section>
    );

    const count = playCard ? 3 : 2;
    return (
        <section class="sc" aria-label={t.label}>
            <div class="sc-head">
                <span class="sc-label">{head}</span>
                <button type="button" class="sc-hide" onClick={hide}>
                    {t.hide}
                </button>
            </div>
            <div
                class="sc-row"
                onScroll={(e) => {
                    const row = e.currentTarget as HTMLDivElement;
                    const step = (row.firstElementChild as HTMLElement | null)?.offsetWidth ?? 0;
                    if (step) setAt(Math.min(count - 1, Math.round(row.scrollLeft / (step + 12))));
                }}
            >
                {first}
                {playCard}
                {makeCard}
            </div>
            <div class="sc-dots" aria-hidden="true">
                {Array.from({ length: count }, (_, index) => (
                    <i key={index} class={index === at ? 'is-on' : undefined} />
                ))}
            </div>
            {failed}
        </section>
    );
}
