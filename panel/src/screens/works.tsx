// «Работы» (docs/plans/router-and-works.md, решение 17, этап 6а): одна страница на все виды работ — приложения
// мастерской (`app`), фоновые инструменты (`tool`), задачи приложений (`task`), работы в рабочих папках (`folder`, этап
// 3). Сверху — «Новая работа» в рабочей папке и вход в «Рабочие папки» (`#/works/folders`, screens/work-folders.tsx:
// папки и агенты — здесь же, решение владельца 16.11.2026). Ниже «Сейчас» — что идёт: вид, строка хода, сколько идёт, на какой модели,
// расход; ниже «Ждёт» — очередь и почему, «Спрашивает» — открытый вопрос и кнопки ответа (те же ответы, что голосом),
// «Недавние» — законченные за месяц, с исходом и расходом. Нажатие ведёт внутрь работы (screens/work.tsx):
// `#/works/<работа>/<вкладка>`. Список — `GET /api/v1/works` с группами шлюза; перечитывается на перемены работ и, пока
// что-то идёт, раз в несколько секунд. Страницу видит администратор: в ней модели и деньги.

import type { JSX } from 'preact';
import { useEffect, useState } from 'preact/hooks';

import { WorkAnswer, WorkGroup, WorkModel, WorkSpent, WorkView, WorkWait, WorkWay, api, can, problemOf } from '../api';
import { agentTitle, setTitle, useHarnesses, useWorkFolders, useWorkSets, WorkFoldersPage } from './work-folders';
import { IconChevronRight } from '../icons';
import { formatDate, formatNumber, tr } from '../i18n';
import { usePoll } from '../poll';
import { elapsed, useSecondTick } from '../shell/agent-plaque';
import { usePanel } from '../shell/context';
import { takeThemeWhenInstalled } from '../shell/theme-apply';
import { timeAgo } from '../time';
import { WorkPage } from './work';

import '../ui/works.css';

/** Недавних сразу видно столько; остальные — по «Показать ещё». */
const RECENT_SHOWN = 12;
/** Пока что-то идёт или ждёт — перечитывать так часто: шаг инструмента, проценты задачи, очередь. */
export const WORKS_POLL_MS = 4000;

// ─── Слова о работе — общие для страницы, работы и виджета ────────────────────────────────────────────────────────

/** Вид работы словами: приложение, инструмент, задача приложения, рабочая папка. */
/** Система работ словами людей: свои имена, остальные как есть. */
export function systemTitle(system: string | undefined): string {
    switch (system) {
        case 'claude-code':
            return 'Claude Code';
        case 'duowave':
            return 'DuoWave';
        default:
            return system ?? tr.router.kindSystem;
    }
}

export function kindWord(kind: string): string {
    switch (kind) {
        case 'app':
            return tr.router.kindApp;
        case 'tool':
            return tr.router.kindTool;
        case 'task':
            return tr.router.kindTask;
        case 'folder':
            return tr.router.kindFolder;
        case 'system':
            return tr.router.kindSystem;
        default:
            return tr.router.kindOther(kind);
    }
}

/** Чем занят агент мастерской — словами пульта. */
function doingWord(doing: string | undefined): string {
    switch (doing) {
        case 'thinking':
            return tr.router.doingThinking;
        case 'writing':
            return tr.router.doingWriting;
        case 'reading':
            return tr.router.doingReading;
        case 'checking':
            return tr.router.doingChecking;
        case 'trying':
            return tr.router.doingTrying;
        case 'phrase':
            return tr.router.doingPhrase;
        case 'docs':
            return tr.router.doingDocs;
        case 'journal':
            return tr.router.doingJournal;
        default:
            return tr.router.doingStarting;
    }
}

/** Строка хода идущей работы: чем занят агент или где фоновое дело словами его исполнителя. */
export function progressLine(work: WorkView): string {
    if (work.kind === 'app') return doingWord(work.doing);
    return work.step?.trim().replace(/\.$/, '') ?? '';
}

/** Часы шлюза — по сдвигу живого канала: время работ пишет шлюз. */
export function gatewayNow(skew: number, at = Date.now()): number {
    return at + skew;
}

/** Почему работа ждёт в очереди. */
export function waitLine(wait: WorkWait | undefined, skew: number): string {
    switch (wait?.why) {
        case 'soon':
            return tr.router.waitSoon(wait.until ? formatDate(wait.until - skew, { hour: '2-digit', minute: '2-digit' }) : '…');
        case 'day':
            return tr.router.waitDay;
        case 'next':
            return tr.router.waitNext;
        default:
            return wait?.ahead ? tr.router.waitBusyAhead(wait.ahead) : tr.router.waitBusy;
    }
}

/** Чем кончилась работа: что стало с вещью, иначе — состояние. */
export function endWord(work: WorkView): string {
    switch (work.result?.end as string | undefined) {
        case 'desk':
            return tr.router.endDesk;
        case 'fitting':
            return tr.router.endFitting;
        case 'parked':
            return tr.router.endParked;
        case 'declined':
            return tr.router.endDeclined;
        case 'removed':
            return tr.router.endRemoved;
    }
    switch (work.state) {
        case 'ready':
            return tr.router.endReady;
        case 'unfinished':
            return tr.router.endUnfinished;
        case 'failed':
            return tr.router.endFailed;
        case 'stopped':
            return tr.router.endStopped;
        case 'interrupted':
            return tr.router.endInterrupted;
        default:
            return tr.router.endDone;
    }
}

/** Точка работы: идёт — агент, спрашивает и готово — своё, не вышло — беда, остальное — камень. */
export function dotOf(work: WorkView): string {
    if (work.group === 'going' || work.state === 'working') return 'wk-dot is-going';
    if (work.group === 'queued' || work.state === 'queued') return 'wk-dot is-off';
    if (work.state === 'failed') return 'wk-dot is-failed';
    if (work.state === 'stopped' || work.state === 'interrupted') return 'wk-dot is-off';
    return 'wk-dot is-own';
}

/** Где думает модель: дома — своё, своё облако и облако — снаружи. */
export function placeWord(place: string | undefined): string | null {
    if (place === 'home') return tr.router.home;
    if (place === 'mine') return tr.router.mine;
    if (place === 'cloud') return tr.router.cloud;
    return null;
}

/** Доллары оценки: мелкое — до сотых цента, крупное — до центов. */
export function dollars(usdMicro: number): string {
    const usd = usdMicro / 1_000_000;
    const digits = usd === 0 ? 2 : usd < 0.01 ? 4 : usd < 1 ? 3 : 2;
    return `$${formatNumber(usd, { minimumFractionDigits: digits, maximumFractionDigits: digits })}`;
}

/** Токены коротко: «14 тыс.», «1,2 млн». */
export function tokensShort(n: number): string {
    return formatNumber(n, { notation: 'compact', maximumFractionDigits: 1 });
}

/**
 * Расход работы одной строкой: доллары и токены её сделок; у идущей без сделок — токены её записи (круги агента
 * считаются по ходу, сделка пишется в конце ответа).
 */
export function spentLine(spent: WorkSpent | undefined, work: WorkView): string | null {
    if (spent && spent.deals > 0) {
        const tokens = spent.tokens.input + spent.tokens.output + spent.tokens.cacheRead + spent.tokens.cacheWrite;
        // Ничего не стоило и всё с ценой — «бесплатно» (дома или бесплатная модель), а не «$0,00».
        const usd = spent.usdMicro === 0 && spent.unpriced === 0 ? tr.router.moneyFree : spent.usdMicro > 0 || spent.unpriced === 0 ? dollars(spent.usdMicro) : tr.router.noPrice;
        return `${usd} · ${tr.router.tokens(tokensShort(tokens))}`;
    }
    const own = work.tokens.input + work.tokens.output + work.tokens.cacheRead + work.tokens.cacheWrite;
    return own > 0 ? tr.router.tokens(tokensShort(own)) : null;
}

/** Модель работы: имя и где она думает. */
export function ModelChip({ model }: { model?: WorkModel }) {
    if (!model) return null;
    const place = placeWord(model.place);
    const tone = model.place === 'home' ? 'pill-own' : model.place ? 'pill-yandex' : 'pill-off';
    return (
        <span class={`pill ${tone} wk-model`} title={`${model.provider} · ${model.model}`}>
            {model.model}
            {place && <span class="wk-model-place">{place}</span>}
        </span>
    );
}

/** Кнопки ответа на вопрос работы по его виду — те же ответы, что голосом; первая — главная. */
export function answersOf(work: WorkView): Array<{ answer: WorkAnswer; label: string; danger?: boolean; to?: string }> {
    // Вопрос лимита предлагает ещё то, что возможно сейчас (этап 5): набор дешевле, другую систему.
    const offers = (work.question?.offers ?? []).map((offer) =>
        offer.kind === 'switch'
            ? { answer: 'switch' as WorkAnswer, label: tr.router.answerSwitch(offer.title) }
            : { answer: 'hand' as WorkAnswer, label: tr.router.answerHand(offer.title), to: offer.id },
    );
    switch (work.question?.kind) {
        case 'ready':
            return [
                { answer: 'yes', label: work.face === 'widget' || !work.face ? tr.router.answerDesk : tr.router.answerEnable },
                { answer: 'fit', label: tr.router.answerFit },
                { answer: 'park', label: tr.router.answerPark },
                { answer: 'no', label: tr.router.answerLater },
            ];
        case 'unfinished':
            return [
                { answer: 'yes', label: tr.router.answerGoOn },
                { answer: 'details', label: tr.router.answerDetails },
                { answer: 'park', label: tr.router.answerPark },
                { answer: 'no', label: tr.router.answerLater },
            ];
        case 'apply':
            return [
                { answer: 'yes', label: tr.router.answerApply },
                { answer: 'no', label: tr.router.answerLater },
            ];
        case 'delete':
            return [
                { answer: 'yes', label: tr.router.answerDelete, danger: true },
                { answer: 'no', label: tr.router.answerKeep },
            ];
        case 'more':
            return [
                { answer: 'yes', label: tr.router.answerDetails },
                { answer: 'no', label: tr.router.answerNoNeed },
            ];
        // Лимит подписки держит работу в папке: подождать (и без ответа ждёт) или остановить.
        case 'wait':
            return [
                { answer: 'yes', label: tr.router.answerWait },
                ...offers,
                { answer: 'no', label: tr.router.answerStop, danger: true },
            ];
        // Бюджет работы в папке кончился: добавить столько же (и работа идёт дальше) или остановить; без ответа ждёт.
        case 'budget':
            return [
                { answer: 'yes', label: tr.router.answerAddBudget },
                ...offers,
                { answer: 'no', label: tr.router.answerStop, danger: true },
            ];
        // «С планом»: план готов — делать по нему или не надо.
        case 'plan':
            return [
                { answer: 'yes', label: tr.router.answerDoPlan },
                { answer: 'no', label: tr.router.answerStop, danger: true },
            ];
        case 'permission':
            return [
                { answer: 'yes', label: tr.router.answerAllow },
                { answer: 'no', label: tr.router.answerDeny, danger: true },
            ];
        default:
            return [];
    }
}

/**
 * Ответы на вопрос работы кнопками: нажатие — `POST /api/v1/works/{id}/do {action: answer}`, ответ шлюза — строкой под
 * кнопками (его же сказали бы голосом). `onAnswered` — перечитать.
 */
export function AnswerButtons({ work, onAnswered }: { work: WorkView; onAnswered: (said: string) => void }) {
    const [busy, setBusy] = useState<WorkAnswer | null>(null);
    const [said, setSaid] = useState<{ text: string; failed: boolean } | null>(null);
    const answers = answersOf(work);
    if (answers.length === 0) return <p class="w-cap m-0">{tr.router.answerAloud}</p>;
    // Ответ шлюза показывает тот, кто перечитывает: на вопрос ответили — работа уходит из «Спрашивает» вместе с кнопками.
    const answer = (choice: WorkAnswer, to?: string) => {
        setBusy(choice);
        setSaid(null);
        // «Применить» / «Включить» у темы: поставленная — сразу на пульте (shell/theme-apply.ts).
        if (choice === 'yes' && work.face === 'theme' && work.target) takeThemeWhenInstalled(work.target);
        api.workAnswer(work.id, choice, to)
            .then((reply) => onAnswered(tr.router.answered(reply.said)))
            .catch((e) => {
                setSaid({ text: problemOf(e), failed: true });
                onAnswered('');
            })
            .finally(() => setBusy(null));
    };
    return (
        <div class="wk-answers">
            <div class="wk-answer-row">
                {answers.map((entry, at) => (
                    <button
                        key={`${entry.answer}${entry.to ?? ''}`}
                        type="button"
                        class={`pill ${entry.danger ? 'pill-error' : at === 0 ? 'pill-own' : ''}`}
                        disabled={busy !== null}
                        onClick={() => answer(entry.answer, entry.to)}
                    >
                        {busy === entry.answer ? '…' : entry.label}
                    </button>
                ))}
            </div>
            {said && (
                <p class="w-cap m-0 c-dot-offline" role="alert">
                    {said.text}
                </p>
            )}
        </div>
    );
}

/** Что шлюз ответил на ответ кнопкой — строкой на несколько секунд: «Часы: на столе». */
export function useSaid(): [string | null, (title: string, said: string) => void] {
    const [said, setSaid] = useState<string | null>(null);
    useEffect(() => {
        if (!said) return;
        const timer = window.setTimeout(() => setSaid(null), 8000);
        return () => window.clearTimeout(timer);
    }, [said]);
    return [said, (title, text) => text && setSaid(tr.router.saidAbout(title, text))];
}

// ─── Страница ───────────────────────────────────────────────────────────────────────────────────────────────────

/** Раздел «Работы»: список или, по адресу `#/works/<работа>`, сама работа. */
export function WorksSection() {
    const panel = usePanel();
    if (!can('settings.edit')) return <p class="w-cap m-0">{tr.router.adminOnly}</p>;
    const id = panel.link.section === 'works' ? panel.link.tab : undefined;
    if (id === 'folders') return <WorkFoldersPage />;
    if (id) return <WorkPage key={id} id={id} />;
    return <WorksList />;
}

/** Работы, перечитанные на перемены и, пока что-то идёт, раз в несколько секунд. */
export function useWorks(groups: WorkGroup[], on = true): { works: WorkView[] | null; problem: string | null; reread: () => void } {
    const panel = usePanel();
    const [works, setWorks] = useState<WorkView[] | null>(null);
    const [problem, setProblem] = useState<string | null>(null);
    const live = !!works?.some((work) => work.group === 'going' || work.group === 'queued');
    const reread = usePoll({
        on,
        every: live ? WORKS_POLL_MS : 0,
        version: panel.version,
        settle: 1000,
        deps: [groups.join(',')],
        read: (signal) =>
            api
                .worksIn(groups, signal)
                .then((list) => {
                    if (signal.aborted) return;
                    setWorks(list);
                    setProblem(null);
                })
                .catch((e) => {
                    if (!signal.aborted) setProblem(problemOf(e));
                }),
    });
    return { works, problem, reread };
}

function WorksList() {
    const panel = usePanel();
    const { works, problem, reread } = useWorks([]);
    const [allRecent, setAllRecent] = useState(false);
    const [said, say] = useSaid();
    const of = (group: WorkGroup) => (works ?? []).filter((work) => work.group === group);
    const going = of('going');
    const queued = of('queued');
    const asking = of('asking');
    const recent = of('recent').sort((a, b) => (b.endedAt ?? b.updatedAt) - (a.endedAt ?? a.updatedAt));
    const tick = useSecondTick(going.length > 0 || queued.length > 0);
    const now = gatewayNow(panel.feed.skew, tick);
    const open = (work: WorkView) => panel.go('works', work.id);
    if (!works)
        return (
            <p class={`w-cap m-0${problem ? ' c-dot-offline' : ''}`} role={problem ? 'alert' : undefined}>
                {problem ?? tr.router.reading}
            </p>
        );
    const shownRecent = allRecent ? recent : recent.slice(0, RECENT_SHOWN);
    return (
        <div class="wk-page">
            <div class="wk-top">
                <p class="wk-summary m-0" role="status">
                    {tr.router.summary(going.length, queued.length, asking.length)}
                </p>
                <button type="button" class="pill" onClick={() => panel.go('works', 'folders')}>
                    {tr.router.foldersOpen}
                </button>
            </div>
            <NewWork onStarted={(work, text) => {
                say(work.title, text);
                reread();
            }} />
            {said && (
                <p class="wk-said m-0" role="status">
                    {said}
                </p>
            )}
            {problem && (
                <p class="w-cap m-0 c-dot-offline" role="alert">
                    {problem}
                </p>
            )}
            <Group id="going" title={tr.router.groupGoing}>
                {going.length === 0 ? (
                    <p class="w-cap m-0 wk-empty">{tr.router.noneGoing}</p>
                ) : (
                    <ul class="wk-rows">
                        {going.map((work) => (
                            <Row key={work.id} work={work} onOpen={open} meta={[progressLine(work), tr.router.since(elapsed(now - work.startedAt)), spentLine(work.spent, work)]} model={work.model} />
                        ))}
                    </ul>
                )}
            </Group>
            {queued.length > 0 && (
                <Group id="queued" title={tr.router.groupQueued}>
                    <ul class="wk-rows">
                        {queued.map((work) => (
                            <Row key={work.id} work={work} onOpen={open} meta={[waitLine(work.wait, panel.feed.skew), tr.router.queuedFor(elapsed(now - work.startedAt))]} />
                        ))}
                    </ul>
                </Group>
            )}
            {asking.length > 0 && (
                <Group id="asking" title={tr.router.groupAsking}>
                    <ul class="wk-rows">
                        {asking.map((work) => (
                            <Row key={work.id} work={work} onOpen={open} lead={work.result?.line ?? work.question?.text} meta={[spentLine(work.spent, work)]} model={work.model}>
                                <AnswerButtons
                                    work={work}
                                    onAnswered={(text) => {
                                        say(work.title, text);
                                        reread();
                                    }}
                                />
                            </Row>
                        ))}
                    </ul>
                </Group>
            )}
            <Group id="recent" title={tr.router.groupRecent}>
                {recent.length === 0 ? (
                    <p class="w-cap m-0 wk-empty">{tr.router.noneRecent}</p>
                ) : (
                    <>
                        <ul class="wk-rows">
                            {shownRecent.map((work) => (
                                <Row
                                    key={work.id}
                                    work={work}
                                    onOpen={open}
                                    meta={[endWord(work), timeAgo(new Date((work.endedAt ?? work.updatedAt) - panel.feed.skew).toISOString()), spentLine(work.spent, work)]}
                                    model={work.model}
                                />
                            ))}
                        </ul>
                        {!allRecent && recent.length > RECENT_SHOWN && (
                            <button type="button" class="w-btn wk-more" onClick={() => setAllRecent(true)}>
                                {tr.router.moreRecent(recent.length - RECENT_SHOWN)}
                            </button>
                        )}
                    </>
                )}
            </Group>
        </div>
    );
}

/**
 * «Новая работа» в рабочей папке: папка, задание, агент (по умолчанию — агент папки), «Запустить». Ответ шлюза — строкой
 * сверху, работа встаёт в «Сейчас» или в очередь. Папок нет — ссылка на «Рабочие папки».
 */
function NewWork({ onStarted }: { onStarted: (work: WorkView, said: string) => void }) {
    const panel = usePanel();
    const { folders, problem: foldersProblem } = useWorkFolders();
    const { harnesses } = useHarnesses();
    const [folder, setFolder] = useState('');
    const [task, setTask] = useState('');
    const [agent, setAgent] = useState('');
    const [set, setSet] = useState('');
    const [way, setWay] = useState<'' | WorkWay>('');
    const sets = useWorkSets();
    const [busy, setBusy] = useState(false);
    const [problem, setProblem] = useState<string | null>(null);
    // Работа в папках есть только у компьютера: шлюз колонки этих папок не знает.
    if (foldersProblem && !folders) return null;
    if (!folders) return null;
    const chosen = folders.find((entry) => entry.id === folder) ?? folders[0];
    const start = () => {
        if (!chosen || !task.trim()) return;
        setBusy(true);
        setProblem(null);
        api.startFolderWork(chosen.id, task.trim(), agent || undefined, undefined, set || undefined, way || undefined)
            .then((begun) => {
                setTask('');
                onStarted(begun.work, begun.said);
            })
            .catch((e) => setProblem(problemOf(e)))
            .finally(() => setBusy(false));
    };
    return (
        <section class="card glass card-list wk-group" aria-labelledby="wk-new-h">
            <div class="card-head">
                <h2 id="wk-new-h" class="t-title-xs m-0">
                    {tr.router.newWork}
                </h2>
            </div>
            {folders.length === 0 ? (
                <p class="w-cap m-0 wk-empty">
                    {tr.router.newWorkNoFolders}{' '}
                    <button type="button" class="sp-link" onClick={() => panel.go('works', 'folders')}>
                        {tr.router.foldersOpen}
                    </button>
                </p>
            ) : (
                <form
                    class="wf-form"
                    onSubmit={(event) => {
                        event.preventDefault();
                        start();
                    }}
                >
                    <div class="wf-pair">
                        <label class="flex flex-col gap-4">
                            <span class="t-caption">{tr.router.newWorkFolder}</span>
                            <select class="field" value={chosen?.id} onChange={(e) => setFolder((e.target as HTMLSelectElement).value)}>
                                {folders.map((entry) => (
                                    <option key={entry.id} value={entry.id}>
                                        {entry.name}
                                    </option>
                                ))}
                            </select>
                        </label>
                        <label class="flex flex-col gap-4">
                            <span class="t-caption">{tr.router.newWorkAgent}</span>
                            <select class="field" value={agent} onChange={(e) => setAgent((e.target as HTMLSelectElement).value)}>
                                <option value="">{tr.router.newWorkAgentOfFolder(agentTitle(harnesses, chosen?.agent))}</option>
                                {(harnesses ?? []).map((harness) => (
                                    <option key={harness.id} value={harness.id}>
                                        {harness.title}
                                    </option>
                                ))}
                            </select>
                        </label>
                    </div>
                    <label class="flex flex-col gap-4">
                        <span class="t-caption">{tr.router.wayLabel}</span>
                        <select class="field" value={way} onChange={(e) => setWay((e.target as HTMLSelectElement).value as '' | WorkWay)}>
                            <option value="">{tr.router.wayAtOnce}</option>
                            <option value="review">{tr.router.wayReview}</option>
                            <option value="plan">{tr.router.wayPlan}</option>
                        </select>
                    </label>
                    {sets && (
                        <label class="flex flex-col gap-4">
                            <span class="t-caption">{tr.router.setLabel}</span>
                            <select class="field" value={set} onChange={(e) => setSet((e.target as HTMLSelectElement).value)}>
                                <option value="">{tr.router.setOfFolder(setTitle(sets, chosen?.set ?? sets.default))}</option>
                                {sets.sets.map((entry) => (
                                    <option key={entry.id} value={entry.id}>
                                        {setTitle(sets, entry.id)}
                                    </option>
                                ))}
                            </select>
                        </label>
                    )}
                    <label class="flex flex-col gap-4">
                        <span class="t-caption">{tr.router.newWorkTask}</span>
                        <textarea
                            class="field wk-note-field"
                            rows={3}
                            maxLength={4000}
                            placeholder={tr.router.newWorkTaskHint}
                            value={task}
                            onInput={(e) => setTask((e.currentTarget as HTMLTextAreaElement).value)}
                            onKeyDown={(e) => {
                                if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) {
                                    e.preventDefault();
                                    start();
                                }
                            }}
                        />
                    </label>
                    {problem && (
                        <p class="w-cap m-0 c-dot-offline" role="alert">
                            {problem}
                        </p>
                    )}
                    <div class="wk-actions">
                        <button type="submit" class="pill pill-own" disabled={busy || !task.trim()}>
                            {busy ? '…' : tr.router.newWorkStart}
                        </button>
                    </div>
                </form>
            )}
        </section>
    );
}

function Group({ id, title, children }: { id: string; title: string; children: JSX.Element | JSX.Element[] | false }) {
    return (
        <section class="card glass card-list wk-group" aria-labelledby={`wk-${id}-h`}>
            <div class="card-head">
                <h2 id={`wk-${id}-h`} class="t-title-xs m-0">
                    {title}
                </h2>
            </div>
            {children}
        </section>
    );
}

/**
 * Строка работы: точка, название и вид, ниже — вопрос и что о ней сказать (ход, очередь, исход, расход), справа — модель.
 * Нажатие — внутрь работы. `children` — под строкой (кнопки ответа), вне кнопки строки.
 */
function Row({
    work,
    lead,
    meta,
    model,
    onOpen,
    children,
}: {
    work: WorkView;
    /** Главная строка под названием: вопрос работы. */
    lead?: string;
    meta: Array<string | null | undefined>;
    model?: WorkModel;
    onOpen: (work: WorkView) => void;
    children?: JSX.Element;
}) {
    const lines = meta.filter((line): line is string => !!line && line.trim() !== '');
    return (
        <li class="wk-row">
            <button type="button" class="wk-row-open" aria-label={tr.router.openWork(work.title)} onClick={() => onOpen(work)}>
                <span class={dotOf(work)} aria-hidden="true" />
                <span class="wk-row-main">
                    <span class="wk-row-title">
                        <b>{work.title}</b>
                        <span class="wk-kind">{work.kind === 'system' ? systemTitle(work.system) : kindWord(work.kind)}</span>
                        {work.outside && <span class="wk-kind">{tr.router.beganOutside}</span>}
                    </span>
                    {lead && <span class="wk-lead">{lead}</span>}
                    {lines.length > 0 && <span class="wk-meta">{lines.join(' · ')}</span>}
                </span>
                <ModelChip model={model} />
                <IconChevronRight size={16} />
            </button>
            {children}
        </li>
    );
}
