// Одна работа (docs/plans/router-and-works.md, «Устройство: пульт», «Работа»; этап 6а) — `#/works/<работа>/<вкладка>`.
// Сверху — что это и где она: вид, состояние, сколько идёт или чем кончилась, модель и расход; «Остановить» и
// «Добавить вдогонку»; открытый вопрос — с кнопками ответа. Ниже вкладки:
// - «Ход» — заявка и заметки вдогонку; у приложения мастерской — шаги агента (`/api/v1/workshop/drafts/{id}/activity`,
//   тот же «Ход работы», что в мастерской); у задачи приложения — её шаги (`/api/v1/tasks/{id}`); у инструмента — где
//   он сейчас; у всех — итог.
// - «Сделки» — сделки роутера этой работы: место, модель, причина, токены, доллары (`/api/v1/works/{id}/deals`); строка
//   сделки — та же, что в «Расходе» (screens/spend.tsx).
// - «Где» — что известно о месте работы: шлюз, где попросили, кто делает, черновик и его папка, сервис или
//   приложение, модель и где она думает.
// Работа в рабочей папке (этап 3): в «Ходе» — агент, модель, строка хода, проверки папки и последнее сообщение агента;
// вкладка «Журнал» (screens/work-journal.tsx) — запуски и журнал выбранного; в «Где» — папка, ветка, рабочая копия,
// сессия агента и команда, которой она открывается в терминале; у кнопок — «Продолжить» и «Добавить вдогонку» (новый
// запуск в той же сессии). Бюджет работы — этап 6в.

import { useEffect, useState } from 'preact/hooks';

import { AppTaskLog, ApiError, DraftView, FolderWorkView, ModelDeal, WorkView, api, problemOf } from '../api';
import { IconChevronLeft } from '../icons';
import { formatDate, tr } from '../i18n';
import { usePoll } from '../poll';
import { elapsed, useSecondTick } from '../shell/agent-plaque';
import { usePanel } from '../shell/context';
import { ask } from '../ui/confirm';
import { SectionTabs } from '../ui/tabs';
import { DealRow, dealsMoney } from './spend';
import { AgentLog } from './workshop';
import { Journal, useFolderRuns } from './work-journal';
import {
    AnswerButtons,
    ModelChip,
    WORKS_POLL_MS,
    dotOf,
    endWord,
    gatewayNow,
    kindWord,
    placeWord,
    progressLine,
    spentLine,
    waitLine,
} from './works';

/** Вкладки работы: у работы в рабочей папке между «Ходом» и «Сделками» — «Журнал» её запусков. */
type WorkTab = 'progress' | 'journal' | 'deals' | 'where';
const TABS: WorkTab[] = ['progress', 'deals', 'where'];
const FOLDER_TABS: WorkTab[] = ['progress', 'journal', 'deals', 'where'];

/** Время шлюза (мс) — по часам этого браузера: «14:05». */
function at(ms: number, skew: number): string {
    return formatDate(ms - skew, { hour: '2-digit', minute: '2-digit' });
}

/** День и время по часам браузера: «15 нояб., 14:05». */
function dayAt(ms: number, skew: number): string {
    return formatDate(ms - skew, { day: 'numeric', month: 'short', hour: '2-digit', minute: '2-digit' });
}

export function WorkPage({ id }: { id: string }) {
    const panel = usePanel();
    const picked = panel.link.item as WorkTab | undefined;
    const [work, setWork] = useState<WorkView | null>(null);
    const tabs = work?.kind === 'folder' ? FOLDER_TABS : TABS;
    const tab: WorkTab = picked && tabs.includes(picked) ? picked : 'progress';
    const [problem, setProblem] = useState<string | null>(null);
    const [missing, setMissing] = useState(false);
    const live = work?.state === 'working' || work?.state === 'queued';
    const reread = usePoll({
        every: live ? WORKS_POLL_MS : 0,
        version: panel.version,
        settle: 1000,
        deps: [id],
        read: (signal) =>
            api
                .work(id, signal)
                .then((fresh) => {
                    if (signal.aborted) return;
                    setWork(fresh);
                    setProblem(null);
                })
                .catch((e) => {
                    if (signal.aborted) return;
                    if (e instanceof ApiError && e.status === 404) setMissing(true);
                    else setProblem(problemOf(e));
                }),
    });
    const back = (
        <button type="button" class="deeper-back" onClick={() => panel.go('works', '')}>
            <IconChevronLeft size={16} />
            {tr.router.back}
        </button>
    );
    if (!work)
        return (
            <div class="wk-page">
                {back}
                <p class={`w-cap m-0${problem || missing ? ' c-dot-offline' : ''}`} role={problem || missing ? 'alert' : undefined}>
                    {missing ? tr.router.notFound : (problem ?? tr.router.reading)}
                </p>
            </div>
        );
    const label = (entry: WorkTab) =>
        entry === 'progress' ? tr.router.tabProgress : entry === 'journal' ? tr.router.tabJournal : entry === 'deals' ? tr.router.tabDeals : tr.router.tabWhere;
    return (
        <div class="wk-page">
            {back}
            <Head work={work} onChanged={reread} />
            <SectionTabs
                label={tr.router.tabsLabel}
                tabs={tabs.map((entry) => ({ id: entry, label: label(entry), icon: null }))}
                tab={tab}
                onPick={(next) => panel.openItem('works', id, next === 'progress' ? null : next, {}, true)}
            />
            {tab === 'progress' && <Progress work={work} />}
            {tab === 'journal' && <Journal work={work} />}
            {tab === 'deals' && <Deals work={work} />}
            {tab === 'where' && <Where work={work} />}
        </div>
    );
}

/** Шапка работы: название и вид, где она, модель и расход, действия и открытый вопрос. */
function Head({ work, onChanged }: { work: WorkView; onChanged: () => void }) {
    const panel = usePanel();
    const going = work.state === 'working';
    const queued = work.state === 'queued';
    const tick = useSecondTick(going || queued);
    const now = gatewayNow(panel.feed.skew, tick);
    const [busy, setBusy] = useState(false);
    const [said, setSaid] = useState<{ text: string; failed: boolean } | null>(null);
    const [noteOpen, setNoteOpen] = useState(false);
    const [note, setNote] = useState('');
    // Вдогонку — работе, что делает вещь (мастерская): пока она идёт или ждёт, или ответом на её вопрос — новая версия.
    // Работе в папке — всегда: идёт — заметка ждёт конца запуска; кончилась — новый запуск в той же сессии.
    const folder = work.kind === 'folder';
    const mayAdd = (work.kind === 'app' && (going || queued || work.group === 'asking')) || folder;
    // «Продолжить» — работе в папке, которую прервали, остановили или которая не доделана: та же сессия агента.
    const mayGoOn = folder && ['interrupted', 'stopped', 'unfinished', 'failed'].includes(work.state);
    const act = (job: () => Promise<{ said: string }>) => {
        setBusy(true);
        setSaid(null);
        job()
            .then((reply) => setSaid({ text: reply.said, failed: false }))
            .catch((e) => setSaid({ text: problemOf(e), failed: true }))
            .finally(() => {
                setBusy(false);
                onChanged();
            });
    };
    const stop = async () => {
        const sure = await ask({
            title: tr.router.stopTitle,
            text: queued ? tr.router.stopQueuedText(work.title) : tr.router.stopText(work.title),
            ok: tr.router.stop,
            danger: true,
        });
        if (sure) act(() => api.workDo(work.id, 'stop'));
    };
    const send = () => {
        const text = note.trim();
        if (!text) return;
        act(() =>
            api.workNotes(work.id, text).then((reply) => {
                setNote('');
                setNoteOpen(false);
                return reply;
            }),
        );
    };
    const when = going
        ? [progressLine(work), tr.router.since(elapsed(now - work.startedAt))]
        : queued
          ? [waitLine(work.wait, panel.feed.skew), tr.router.queuedFor(elapsed(now - work.startedAt))]
          : [endWord(work), work.endedAt ? dayAt(work.endedAt, panel.feed.skew) : null];
    const lines = [...when, spentLine(work.spent, work)].filter((line): line is string => !!line && line.trim() !== '');
    return (
        <section class="card glass wk-head" aria-labelledby="wk-head-h">
            <div class="wk-head-top">
                <span class={dotOf(work)} aria-hidden="true" />
                <div class="wk-row-main">
                    <div class="wk-row-title">
                        <h2 id="wk-head-h" class="wk-head-title m-0">
                            {work.title}
                        </h2>
                        <span class="wk-kind">{kindWord(work.kind)}</span>
                    </div>
                    <span class="wk-meta">{lines.join(' · ')}</span>
                    <ModelChip model={work.model} />
                </div>
            </div>
            {work.group === 'asking' && (
                <div class="wk-question">
                    <p class="m-0">{work.result?.line ?? work.question?.text}</p>
                    <AnswerButtons
                        work={work}
                        onAnswered={(text) => {
                            if (text) setSaid({ text, failed: false });
                            onChanged();
                        }}
                    />
                </div>
            )}
            {(going || queued || mayAdd || mayGoOn) && !noteOpen && (
                <div class="wk-actions">
                    {(going || queued) && (
                        <button type="button" class="pill" disabled={busy} onClick={() => void stop()}>
                            {tr.router.stop}
                        </button>
                    )}
                    {mayGoOn && (
                        <button type="button" class="pill pill-own" disabled={busy} onClick={() => act(() => goOn(work, (id) => panel.go('works', id)))}>
                            {tr.router.goOn}
                        </button>
                    )}
                    {mayAdd && (
                        <button type="button" class="pill" disabled={busy} onClick={() => setNoteOpen(true)}>
                            {tr.router.addNote}
                        </button>
                    )}
                </div>
            )}
            {noteOpen && (
                <form
                    class="wk-note-form"
                    onSubmit={(event) => {
                        event.preventDefault();
                        send();
                    }}
                >
                    <textarea
                        class="field wk-note-field"
                        rows={2}
                        maxLength={1000}
                        placeholder={tr.router.notePlaceholder}
                        aria-label={tr.router.notePlaceholder}
                        value={note}
                        autoFocus
                        onInput={(event) => setNote((event.currentTarget as HTMLTextAreaElement).value)}
                        onKeyDown={(event) => {
                            if (event.key === 'Enter' && !event.shiftKey) {
                                event.preventDefault();
                                send();
                            }
                        }}
                    />
                    <div class="wk-actions">
                        <button type="submit" class="pill pill-own" disabled={busy || !note.trim()}>
                            {busy ? '…' : tr.router.noteSend}
                        </button>
                        <button type="button" class="pill" disabled={busy} onClick={() => setNoteOpen(false)}>
                            {tr.router.noteCancel}
                        </button>
                    </div>
                </form>
            )}
            {said && (
                <p class={`w-cap m-0${said.failed ? ' c-dot-offline' : ''}`} role={said.failed ? 'alert' : 'status'}>
                    {said.text}
                </p>
            )}
        </section>
    );
}

/**
 * «Продолжить» работу в папке: шлюз заводит ей новую запись с той же целью (та же сессия агента) — страница переходит на
 * неё.
 */
async function goOn(work: WorkView, open: (id: string) => void): Promise<{ said: string }> {
    const reply = await api.workDo(work.id, 'continue');
    const fresh = (await api.worksIn([]).catch(() => [] as WorkView[]))
        .filter((other) => other.id !== work.id && other.target === work.target && other.executor === work.executor)
        .sort((a, b) => b.startedAt - a.startedAt)[0];
    if (fresh) open(fresh.id);
    return reply;
}

// ─── «Ход» ──────────────────────────────────────────────────────────────────────────────────────────────────────

/** Где просили: этот компьютер или колонка по имени. */
function askedWhere(device: string, stations: { device: string; name: string }[] | null): string {
    if (device === 'computer') return tr.router.whereComputer;
    return tr.router.whereStation(stations?.find((station) => station.device === device)?.name ?? device);
}

function Progress({ work }: { work: WorkView }) {
    const panel = usePanel();
    const skew = panel.feed.skew;
    const how = work.origin.activator === 'text' ? tr.router.howText : work.origin.via === 'voice' ? tr.router.howVoice : tr.router.howButton;
    return (
        <>
            <section class="card glass card-list" aria-labelledby="wk-asked-h">
                <div class="card-head">
                    <h3 id="wk-asked-h" class="t-title-xs m-0">
                        {tr.router.asked}
                    </h3>
                </div>
                {/* У приложения мастерской заявка — всё желание; у фонового дела — его название, а `brief` — сервис и
                    инструмент или приложение задачи. */}
                <p class="wk-brief m-0">{work.kind === 'app' || work.kind === 'folder' ? work.brief : work.title}</p>
                {work.kind === 'tool' && <p class="w-cap m-0">{work.brief}</p>}
                {work.kind === 'task' && <p class="w-cap m-0">{tr.router.taskOf(work.brief)}</p>}
                <p class="w-cap m-0 wk-under">{tr.router.askedHow(how, askedWhere(work.origin.device, panel.stations), dayAt(work.startedAt, skew))}</p>
                {work.notes.length > 0 && (
                    <>
                        <h4 class="wk-sub m-0">{tr.router.notes}</h4>
                        <ul class="wk-notes">
                            {work.notes.map((note) => (
                                <li key={note.at} class="wk-note">
                                    <span class="w-cap wk-time">{at(note.at, skew)}</span>
                                    <span class="wk-note-text">+ {note.text}</span>
                                    <span class="w-cap">{note.readAt ? tr.router.noteRead : tr.router.noteUnread}</span>
                                </li>
                            ))}
                        </ul>
                    </>
                )}
            </section>
            {work.kind === 'app' && work.target && <AgentLog id={work.target} title={work.title} />}
            {work.kind === 'folder' && <FolderProgress work={work} />}
            {work.kind === 'task' && work.target && <TaskSteps id={work.target} going={work.state === 'working'} />}
            {work.kind === 'tool' && work.state === 'working' && work.step && (
                <section class="card glass card-list" aria-labelledby="wk-tool-h">
                    <div class="card-head">
                        <h3 id="wk-tool-h" class="t-title-xs m-0">
                            {tr.router.toolStep}
                        </h3>
                    </div>
                    <p class="m-0 wk-brief">{work.step}</p>
                </section>
            )}
            <Result work={work} />
        </>
    );
}

/** Работа в рабочей папке: агент и модель, строка хода, проверки папки, последнее сообщение агента. */
function FolderProgress({ work }: { work: WorkView }) {
    const { folder } = useFolderRuns(work);
    if (!folder) return null;
    const checks = folder.checks ?? [];
    return (
        <section class="card glass card-list" aria-labelledby="wk-folder-h">
            <div class="card-head">
                <h3 id="wk-folder-h" class="t-title-xs m-0">
                    {folder.agentTitle}
                </h3>
                <span class="w-cap">{[folder.folderName, folder.model].filter(Boolean).join(' · ')}</span>
            </div>
            {work.state === 'working' && work.step && <p class="m-0 wk-brief">{work.step}</p>}
            {(checks.length > 0 || folder.checksSkipped) && (
                <>
                    <h4 class="wk-sub m-0">{tr.router.folderChecksTitle}</h4>
                    {folder.checksSkipped ? (
                        <p class="w-cap m-0">{tr.router.choreChecksSkipped}</p>
                    ) : (
                        <ul class="wk-steps">
                            {checks.map((check) => (
                                <li key={check.command} class="wk-step">
                                    <span class={`wk-dot ${check.ok ? 'is-own' : 'is-failed'}`} aria-hidden="true" />
                                    <span class="wk-step-text">
                                        <code class="wk-code">{check.command}</code> — {check.ok ? tr.router.checkPassed : tr.router.checkFailed}
                                        {!check.ok && check.tail && (
                                            <details class="wk-data">
                                                <summary>{tr.router.toolOutput}</summary>
                                                <pre>{check.tail}</pre>
                                            </details>
                                        )}
                                    </span>
                                </li>
                            ))}
                        </ul>
                    )}
                </>
            )}
            {folder.last && (
                <>
                    <h4 class="wk-sub m-0">{tr.router.folderLast}</h4>
                    <p class="m-0 wk-brief">{folder.last}</p>
                </>
            )}
        </section>
    );
}

/** Шаги задачи приложения: проценты и слова по времени; пока идёт — перечитываются. */
function TaskSteps({ id, going }: { id: string; going: boolean }) {
    const panel = usePanel();
    const [task, setTask] = useState<AppTaskLog | null>(null);
    usePoll({
        every: going ? 2000 : 0,
        version: panel.version,
        deps: [id],
        read: (signal) =>
            api
                .task(id)
                .then((fresh) => !signal.aborted && setTask(fresh))
                .catch(() => undefined),
    });
    if (!task || task.log.length === 0) return null;
    return (
        <section class="card glass card-list" aria-labelledby="wk-task-h">
            <div class="card-head">
                <h3 id="wk-task-h" class="t-title-xs m-0">
                    {tr.router.taskLog}
                </h3>
            </div>
            <ol class="wk-steps">
                {task.log.map((step, index) => (
                    <li key={index} class="wk-step">
                        <span class="w-cap wk-time">{at(step.at, panel.feed.skew)}</span>
                        <span class="wk-step-text">
                            {step.percent !== undefined && <b>{step.percent}% </b>}
                            {step.text}
                        </span>
                    </li>
                ))}
            </ol>
            {task.error && (
                <p class="w-cap m-0 c-dot-offline" role="alert">
                    {task.error}
                </p>
            )}
        </section>
    );
}

/** Итог работы: строка шлюза, пересказ и то, что пришло (фоновые дела). */
function Result({ work }: { work: WorkView }) {
    const result = work.result;
    if (!result) return null;
    const data = result.data && result.data.length > 4000 ? `${result.data.slice(0, 4000)}…` : result.data;
    return (
        <section class="card glass card-list" aria-labelledby="wk-result-h">
            <div class="card-head">
                <h3 id="wk-result-h" class="t-title-xs m-0">
                    {tr.router.result}
                </h3>
                <span class="w-cap">{endWord(work)}</span>
            </div>
            <p class="m-0 wk-brief">{result.line}</p>
            {result.summary && (
                <>
                    <h4 class="wk-sub m-0">{tr.router.retold}</h4>
                    <p class="m-0 wk-brief">{result.summary}</p>
                </>
            )}
            {data && (
                <details class="wk-data">
                    <summary>{tr.router.data}</summary>
                    <pre>{data}</pre>
                </details>
            )}
        </section>
    );
}

// ─── «Сделки» ───────────────────────────────────────────────────────────────────────────────────────────────────

function Deals({ work }: { work: WorkView }) {
    const panel = usePanel();
    const [deals, setDeals] = useState<ModelDeal[] | null>(null);
    const [problem, setProblem] = useState<string | null>(null);
    usePoll({
        version: panel.version,
        settle: 1500,
        deps: [work.id],
        read: (signal) =>
            api
                .workDeals(work.id, signal)
                .then((answer) => {
                    if (signal.aborted) return;
                    setDeals(answer.deals);
                    setProblem(null);
                })
                .catch((e) => !signal.aborted && setProblem(problemOf(e))),
    });
    const skew = panel.feed.skew;
    return (
        <section class="card glass card-list" aria-labelledby="wk-deals-h">
            <div class="card-head">
                <h3 id="wk-deals-h" class="t-title-xs m-0">
                    {tr.router.tabDeals}
                </h3>
                {deals && deals.length > 0 && (
                    <span class="w-cap">
                        {tr.router.dealsTotal(deals.length)} · {dealsMoney(deals)}
                    </span>
                )}
            </div>
            <p class="w-cap m-0 wk-under">{tr.router.dealsAbout}</p>
            {problem && (
                <p class="w-cap m-0 c-dot-offline" role="alert">
                    {problem}
                </p>
            )}
            {!deals ? (
                !problem && <p class="w-cap m-0">{tr.router.reading}</p>
            ) : deals.length === 0 ? (
                <p class="w-cap m-0 wk-empty">{work.state === 'working' ? tr.router.noDealsGoing : tr.router.noDeals}</p>
            ) : (
                <ul class="wk-rows">
                    {deals.map((deal) => (
                        <DealRow key={deal.id} deal={deal} skew={skew} />
                    ))}
                </ul>
            )}
        </section>
    );
}

// ─── «Где» ──────────────────────────────────────────────────────────────────────────────────────────────────────

/** «Скопировать» у пути и имени: путь черновика открывают в терминале или редакторе. */
function CopyButton({ text }: { text: string }) {
    const [done, setDone] = useState(false);
    const copy = () =>
        void navigator.clipboard
            ?.writeText(text)
            .then(() => {
                setDone(true);
                window.setTimeout(() => setDone(false), 1500);
            })
            .catch(() => undefined);
    return (
        <button type="button" class="w-btn wk-copy" onClick={copy}>
            {done ? tr.router.copied : tr.router.copy}
        </button>
    );
}

/** «Где» работы в папке: папка, ветка, откуда начата, рабочая копия, агент, сессия и команда для терминала. */
function folderRows(folder: FolderWorkView): Array<[string, string, boolean?]> {
    const rows: Array<[string, string, boolean?]> = [
        [tr.router.whereFolder, folder.folderName],
        [tr.router.whereBranch, folder.fetched ? folder.branch : `${folder.branch} · ${tr.router.whereBranchNotYet}`, true],
    ];
    if (folder.base) rows.push([tr.router.whereBase, folder.baseCommit ? `${folder.base} · ${folder.baseCommit.slice(0, 8)}` : folder.base]);
    rows.push(folder.copy ? [tr.router.whereCopy, folder.copy, true] : [tr.router.whereCopy, tr.router.whereCopyGone]);
    rows.push([tr.router.whereAgent, folder.agentTitle]);
    if (folder.billing) rows.push([tr.router.wherePays, folder.billing === 'plan' ? tr.router.paysPlan : folder.billing === 'key' ? tr.router.paysKey : folder.billing]);
    if (folder.session) rows.push([tr.router.whereSession, folder.session, true]);
    if (folder.resumeCommand) rows.push([tr.router.whereResume, folder.resumeCommand, true]);
    return rows;
}

function Where({ work }: { work: WorkView }) {
    const panel = usePanel();
    const { folder } = useFolderRuns(work);
    const [draft, setDraft] = useState<DraftView | null>(null);
    const [dir, setDir] = useState<string | null>(null);
    const target = work.kind === 'app' ? work.target : undefined;
    useEffect(() => {
        if (!target) return;
        let alive = true;
        api.workshopDraft(target)
            .then((view) => alive && setDraft(view))
            .catch(() => undefined);
        api.workshop()
            .then((view) => alive && setDir(view.dir))
            .catch(() => undefined);
        return () => {
            alive = false;
        };
    }, [target]);
    const station = panel.health?.placement?.role === 'station';
    const who =
        work.kind === 'app'
            ? tr.router.whoWorkshop
            : work.kind === 'tool'
              ? tr.router.whoTool
              : work.kind === 'task'
                ? tr.router.whoTask
                : work.kind === 'folder'
                  ? tr.router.whoFolder
                  : work.executor;
    const rows: Array<[string, string, boolean?]> = [
        [tr.router.whereNumber, tr.router.whereNumberValue(work.id)],
        [tr.router.whereGateway, station ? tr.router.whereGatewayStation : tr.router.whereGatewayPc],
        [tr.router.whereAsked, askedWhere(work.origin.device, panel.stations)],
        [tr.router.whereWho, who],
    ];
    if (work.kind === 'app' && target) {
        rows.push([tr.router.whereDraft, target, true]);
        if (dir) rows.push([tr.router.whereDraftDir, `${dir.replace(/\/$/, '')}/${target}`, true]);
        if (draft) rows.push([tr.router.whereOnDesk, draft.installed ? tr.router.whereOnDeskYes(draft.installed) : tr.router.whereOnDeskNo]);
    }
    if (work.kind === 'folder' && folder) rows.push(...folderRows(folder));
    if (work.kind === 'tool') rows.push([tr.router.whereService, work.brief]);
    if (work.kind === 'task') {
        rows.push([tr.router.whereApp, work.brief, true]);
        if (work.target) rows.push([tr.router.whereTask, work.target, true]);
    }
    rows.push([tr.router.whereModel, work.model ? `${work.model.model} · ${work.model.provider}` : tr.router.whereModelNone]);
    const place = placeWord(work.model?.place);
    if (place) rows.push([tr.router.wherePlace, place]);
    return (
        <section class="card glass card-list" aria-labelledby="wk-where-h">
            <div class="card-head">
                <h3 id="wk-where-h" class="t-title-xs m-0">
                    {tr.router.tabWhere}
                </h3>
            </div>
            {work.kind === 'folder' && folder?.resumeCommand && <p class="w-cap m-0 wk-under">{tr.router.whereResumeHint}</p>}
            <dl class="wk-facts">
                {rows.map(([name, value, code]) => (
                    <div key={name} class="wk-fact">
                        <dt class="w-cap">{name}</dt>
                        <dd class="m-0 wk-fact-value">
                            {code ? <code class="wk-code">{value}</code> : value}
                            {code && <CopyButton text={value} />}
                        </dd>
                    </div>
                ))}
            </dl>
        </section>
    );
}
