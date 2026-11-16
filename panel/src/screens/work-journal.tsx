// Вкладка «Журнал» работы в рабочей папке (docs/plans/router-and-works.md, решение 20, этап 3): запуски работы и журнал
// выбранного — что агент пишет и думает, каждый вызов с файлом или командой и выводом, правки разницей «было / стало»,
// шаги его помощников вложенно, ошибки и отказы, свои шаги шлюза (копия, подготовка, коммит, ветка в папку, проверки).
// Идущий запуск дописывается вживую (`…/log?after=`); «Только ошибки» и поиск — у шлюза; «Скачать как есть» — сырой
// файл запуска. Журнал местный и только администратору: в нём куски кода.

import type { ComponentChildren } from 'preact';
import { useEffect, useMemo, useRef, useState } from 'preact/hooks';

import { FolderRun, FolderWorkView, RunStep, WorkView, api, downloadRunRaw, problemOf } from '../api';
import { formatDate, formatNumber, tr } from '../i18n';
import { usePoll } from '../poll';
import { usePanel } from '../shell/context';
import { tokensShort } from './works';

/** Пока запуск идёт, журнал дочитывается так часто. */
const LIVE_MS = 1500;

/** Работа в папке у помощника и её запуски; пока идёт — перечитываются. */
export function useFolderRuns(work: WorkView): { folder: FolderWorkView | null; runs: FolderRun[]; problem: string | null } {
    const panel = usePanel();
    const [folder, setFolder] = useState<FolderWorkView | null>(null);
    const [runs, setRuns] = useState<FolderRun[]>([]);
    const [problem, setProblem] = useState<string | null>(null);
    const going = work.state === 'working' || work.state === 'queued';
    usePoll({
        on: work.kind === 'folder',
        every: going ? 3000 : 0,
        version: panel.version,
        settle: 800,
        deps: [work.id, work.state],
        read: (signal) =>
            api
                .workRuns(work.id, signal)
                .then((answer) => {
                    if (signal.aborted) return;
                    setFolder(answer.work);
                    setRuns(answer.runs);
                    setProblem(null);
                })
                .catch((e) => !signal.aborted && setProblem(problemOf(e))),
    });
    return { folder, runs, problem };
}

function runWhat(run: FolderRun): string {
    return run.what === 'note' ? tr.router.runNote : run.what === 'resume' ? tr.router.runResume : tr.router.runWork;
}

export function Journal({ work }: { work: WorkView }) {
    const { runs, problem } = useFolderRuns(work);
    const [picked, setPicked] = useState<number | null>(null);
    const [trouble, setTrouble] = useState(false);
    const [find, setFind] = useState('');
    const last = runs.length > 0 ? runs[runs.length - 1].n : null;
    const n = picked ?? last;
    const run = runs.find((entry) => entry.n === n);
    return (
        <section class="card glass card-list" aria-labelledby="wj-h">
            <div class="card-head">
                <h3 id="wj-h" class="t-title-xs m-0">
                    {tr.router.tabJournal}
                </h3>
                {run && (
                    <button type="button" class="w-btn" onClick={() => void downloadRunRaw(work.id, run.n).catch(() => undefined)}>
                        {tr.router.downloadRaw}
                    </button>
                )}
            </div>
            <p class="w-cap m-0 wk-under">{tr.router.journalLocal}</p>
            {problem && (
                <p class="w-cap m-0 c-dot-offline" role="alert">
                    {problem}
                </p>
            )}
            {runs.length > 1 && (
                <div class="wj-runs" role="tablist" aria-label={tr.router.runsTitle}>
                    {runs.map((entry) => (
                        <button
                            key={entry.n}
                            type="button"
                            role="tab"
                            aria-selected={entry.n === n}
                            class={`pill${entry.n === n ? ' pill-pick' : ''}`}
                            onClick={() => setPicked(entry.n)}
                        >
                            {tr.router.runOf(entry.n, runWhat(entry))}
                            {!entry.endedAt && ` · ${tr.router.runGoing}`}
                        </button>
                    ))}
                </div>
            )}
            <div class="wj-filters">
                <label class="wj-check">
                    <input type="checkbox" checked={trouble} onChange={(e) => setTrouble((e.target as HTMLInputElement).checked)} />
                    {tr.router.onlyTrouble}
                </label>
                <input
                    class="field wj-find"
                    type="search"
                    placeholder={tr.router.journalFind}
                    aria-label={tr.router.journalFind}
                    value={find}
                    onInput={(e) => setFind((e.target as HTMLInputElement).value)}
                />
            </div>
            {!run ? (
                !problem && <p class="w-cap m-0 wk-empty">{runs.length === 0 ? tr.router.journalEmpty : tr.router.reading}</p>
            ) : (
                <RunSteps key={`${work.id}:${run.n}`} work={work.id} run={run} trouble={trouble} find={find.trim()} />
            )}
        </section>
    );
}

/** Шаги одного запуска: с начала, потом дочитываются с `next`, пока запуск идёт. */
function RunSteps({ work, run, trouble, find }: { work: string; run: FolderRun; trouble: boolean; find: string }) {
    const panel = usePanel();
    const [steps, setSteps] = useState<RunStep[] | null>(null);
    const [going, setGoing] = useState(!run.endedAt);
    const [problem, setProblem] = useState<string | null>(null);
    const next = useRef(0);
    /** Чем отобраны шаги, что уже прочитаны: другой отбор — читать с начала. */
    const asked = useRef<string | null>(null);
    const [debounced, setDebounced] = useState(find);
    useEffect(() => {
        const timer = window.setTimeout(() => setDebounced(find), 300);
        return () => window.clearTimeout(timer);
    }, [find]);
    usePoll({
        every: going ? LIVE_MS : 0,
        version: panel.version,
        deps: [work, run.n, trouble, debounced],
        read: (signal) => {
            const query = `${trouble}:${debounced}`;
            const fresh = asked.current !== query;
            if (fresh) next.current = 0;
            return api
                .workLog(work, run.n, { after: next.current, trouble, find: debounced || undefined }, signal)
                .then((read) => {
                    if (signal.aborted) return;
                    asked.current = query;
                    next.current = read.next;
                    setSteps((known) => (fresh ? read.steps : [...(known ?? []), ...read.steps]));
                    setGoing(read.going);
                    setProblem(null);
                })
                .catch((e) => !signal.aborted && setProblem(problemOf(e)));
        },
    });
    // Вызов и его продолжение — одна строка: последний шаг с тем же `id` дополняет первый.
    const shown = useMemo(() => merged(steps ?? []), [steps]);
    if (problem)
        return (
            <p class="w-cap m-0 c-dot-offline" role="alert">
                {problem}
            </p>
        );
    if (!steps) return <p class="w-cap m-0">{tr.router.reading}</p>;
    return (
        <>
            {going && <p class="w-cap m-0 wj-live">{tr.router.journalLive}</p>}
            {run.cut && <p class="w-cap m-0">{tr.router.journalCut}</p>}
            {shown.length === 0 ? (
                <p class="w-cap m-0 wk-empty">{trouble || find ? tr.router.journalNothingFound : tr.router.journalEmpty}</p>
            ) : (
                <ol class="wj-steps">
                    {shown.map((step) => (
                        <Step key={step.n} step={step} skew={panel.feed.skew} />
                    ))}
                </ol>
            )}
        </>
    );
}

/** Шаги с вызовами, сложенными по `id`: место — первого, состояние, вывод и разница — последнего. */
function merged(steps: RunStep[]): RunStep[] {
    const out: RunStep[] = [];
    const at = new Map<string, number>();
    for (const step of steps) {
        if (step.type === 'tool' && step.id) {
            const known = at.get(step.id);
            if (known !== undefined) {
                const first = out[known];
                out[known] = { ...first, ...step, n: first.n, at: first.at, diff: step.diff?.length ? step.diff : first.diff, output: step.output ?? first.output };
                continue;
            }
            at.set(step.id, out.length);
        }
        out.push(step);
    }
    return out;
}

function time(ms: number, skew: number): string {
    return formatDate(ms - skew, { hour: '2-digit', minute: '2-digit', second: '2-digit' });
}

function toolVerb(kind: string | undefined): string {
    switch (kind) {
        case 'read':
            return tr.router.toolRead;
        case 'edit':
            return tr.router.toolEdit;
        case 'delete':
            return tr.router.toolDelete;
        case 'move':
            return tr.router.toolMove;
        case 'search':
            return tr.router.toolSearch;
        case 'execute':
            return tr.router.toolExecute;
        case 'think':
            return tr.router.toolThink;
        case 'fetch':
            return tr.router.toolFetch;
        case 'switch_mode':
            return tr.router.toolSwitch;
        default:
            return tr.router.toolOther;
    }
}

function choreWord(chore: RunStep['chore']): string {
    switch (chore) {
        case 'copy':
            return tr.router.choreCopy;
        case 'prepare':
            return tr.router.chorePrepare;
        case 'commit':
            return tr.router.choreCommit;
        case 'fetch':
            return tr.router.choreFetch;
        default:
            return tr.router.choreCheck;
    }
}

/** Что сказать о шаге одной строкой — и что показать под ней. */
function Step({ step, skew }: { step: RunStep; skew: number }) {
    const nested = !!step.parent;
    const failed = (step.type === 'tool' && step.state === 'failed') || (step.type === 'chore' && step.ok === false) || step.type === 'denied' || (step.type === 'asks' && step.allowed === false) || (step.type === 'end' && step.outcome !== 'ok');
    const cls = `wj-step${nested ? ' is-nested' : ''}${failed ? ' is-trouble' : ''} is-${step.type}`;
    let line: string;
    let body: ComponentChildren = null;
    switch (step.type) {
        case 'begin':
            line = tr.router.stepBegin(step.model ?? '—');
            if (step.prompt)
                body = (
                    <details class="wj-more">
                        <summary>{tr.router.stepPrompt}</summary>
                        <pre>{step.prompt}</pre>
                    </details>
                );
            break;
        case 'says':
            line = tr.router.stepSays;
            body = <p class="m-0 wj-text">{step.text}</p>;
            break;
        case 'thinks':
            line = tr.router.stepThinks;
            body = <p class="m-0 wj-text wj-thought">{step.text}</p>;
            break;
        case 'tool': {
            const what = step.command ?? step.path ?? step.title ?? '';
            line = `${toolVerb(step.kind)} ${what}${step.state === 'failed' ? ` · ${tr.router.toolFailed}` : step.state === 'in_progress' || step.state === 'pending' ? ' …' : ''}`;
            const diff = step.diff ?? [];
            if (step.output || diff.length > 0)
                body = (
                    <details class="wj-more" open={step.state === 'failed'}>
                        <summary>{diff.length > 0 ? `${tr.router.diffWas} / ${tr.router.diffNow}` : tr.router.toolOutput}</summary>
                        {diff.map((change, index) => (
                            <div key={index} class="wj-diff">
                                <code class="wk-code">{change.path}</code>
                                {change.old !== undefined && <pre class="wj-was">{change.old}</pre>}
                                <pre class="wj-now">{change.new}</pre>
                            </div>
                        ))}
                        {step.output && <pre>{step.output}</pre>}
                    </details>
                );
            break;
        }
        case 'plan':
            line = tr.router.stepPlan;
            body = (
                <ul class="wj-plan">
                    {(step.entries ?? []).map((entry, index) => (
                        <li key={index} class={`is-${entry.state}`}>
                            {entry.text}
                        </li>
                    ))}
                </ul>
            );
            break;
        case 'usage': {
            const tokens = step.tokens ? step.tokens.input + step.tokens.output + step.tokens.cacheRead + step.tokens.cacheWrite : 0;
            const parts = [tokens ? tr.router.stepUsage(tr.router.tokens(tokensShort(tokens))) : null, step.used && step.size ? tr.router.stepContext(tokensShort(step.used), tokensShort(step.size)) : null];
            line = parts.filter(Boolean).join(' · ') || tr.router.stepUsage('—');
            break;
        }
        case 'retry':
            line = step.waitMs ? tr.router.stepRetryIn(step.reason ?? '', formatNumber(Math.round(step.waitMs / 1000))) : tr.router.stepRetry(step.reason ?? '');
            break;
        case 'limit':
            line = step.until ? tr.router.stepLimit(formatDate(step.until - skew, { hour: '2-digit', minute: '2-digit' })) : tr.router.stepLimitUnknown;
            break;
        case 'denied':
            line = tr.router.stepDenied(step.what || step.tool || '');
            break;
        case 'asks':
            line = tr.router.stepAsked(step.what || step.tool || '', !!step.allowed);
            break;
        case 'chore':
            line = step.chore === 'check' && !step.command ? tr.router.choreChecksSkipped : `${choreWord(step.chore)}${step.command ? `: ${step.command}` : ''}${step.ok === false ? ` · ${tr.router.toolFailed}` : ''}`;
            if (step.output && step.chore !== 'copy' && step.output !== 'skipped')
                body = (
                    <details class="wj-more" open={step.ok === false}>
                        <summary>{tr.router.toolOutput}</summary>
                        <pre>{step.output}</pre>
                    </details>
                );
            break;
        case 'cut':
            line = tr.router.stepCut;
            break;
        default:
            line = tr.router.stepEnd;
            body = (
                <>
                    {step.text && <p class="m-0 wj-text">{step.text}</p>}
                    {step.error && <p class="m-0 wj-text c-dot-offline">{step.error}</p>}
                </>
            );
    }
    return (
        <li class={cls}>
            <span class="w-cap wk-time">{time(step.at, skew)}</span>
            <div class="wj-step-main">
                <span class="wj-line">
                    {nested && <span class="wj-sub-agent">{tr.router.subagent} · </span>}
                    {line}
                </span>
                {body}
            </div>
        </li>
    );
}
