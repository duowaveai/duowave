// «Рабочие папки» (docs/plans/router-and-works.md, «Устройство: пульт», этап 3) — `#/works/folders`, со страницы
// «Работы» (решение владельца 16.11.2026: папки и работы — в одном месте). Папки на этом компьютере, где агент делает
// работы: добавить (только папка с git), агент и модель папки, потолок запуска, подготовка, проверки, разрешённые
// команды; убрать из списка. Рядом — «Агенты»: Claude Code и поставленные карточки — найден ли, версия, вошёл ли; у
// Claude Code — чем платит: «Как в терминале» или «По ключу API». Свою возможность именем Claude Code не называем:
// место — «Агенты», а Claude Code — один из них, обычным текстом.

import { useEffect, useState } from 'preact/hooks';

import { ApiError, HarnessView, ProviderView, WorkFolderInput, WorkFolderView, api, problemOf } from '../api';
import { IconChevronLeft } from '../icons';
import { tr } from '../i18n';
import { usePoll } from '../poll';
import { usePanel } from '../shell/context';
import { ask } from '../ui/confirm';

/** Агенты шлюза — для папок и «Новой работы»; null — ещё не прочитаны, `problem` — не прочитать. */
export function useHarnesses(): { harnesses: HarnessView[] | null; problem: string | null; reread: () => void; set: (fresh: HarnessView) => void } {
    const panel = usePanel();
    const [harnesses, setHarnesses] = useState<HarnessView[] | null>(null);
    const [problem, setProblem] = useState<string | null>(null);
    const reread = usePoll({
        version: panel.version,
        settle: 2000,
        read: (signal) =>
            api
                .harnesses(signal)
                .then((answer) => {
                    if (signal.aborted) return;
                    setHarnesses(answer.harnesses);
                    setProblem(null);
                })
                .catch((e) => !signal.aborted && setProblem(problemOf(e))),
    });
    const set = (fresh: HarnessView) => setHarnesses((all) => (all ?? []).map((harness) => (harness.id === fresh.id ? fresh : harness)));
    return { harnesses, problem, reread, set };
}

/** Рабочие папки шлюза. */
export function useWorkFolders(): { folders: WorkFolderView[] | null; problem: string | null; reread: () => void } {
    const panel = usePanel();
    const [folders, setFolders] = useState<WorkFolderView[] | null>(null);
    const [problem, setProblem] = useState<string | null>(null);
    const reread = usePoll({
        version: panel.version,
        settle: 1500,
        read: (signal) =>
            api
                .workFolders(signal)
                .then((answer) => {
                    if (signal.aborted) return;
                    setFolders(answer.folders);
                    setProblem(null);
                })
                // Шлюз колонки работ в папках не знает: они — только на компьютере.
                .catch((e) => !signal.aborted && setProblem(e instanceof ApiError && e.status === 404 ? tr.router.foldersOnlyHere : problemOf(e))),
    });
    return { folders, problem, reread };
}

/** Строки поля «по одной в строке» и обратно. */
const lines = (text: string): string[] =>
    text
        .split('\n')
        .map((line) => line.trim())
        .filter(Boolean);

/** Агент по id словами: имя карточки или сам id. */
export function agentTitle(harnesses: HarnessView[] | null, id: string | undefined): string {
    if (!id) return harnesses?.find((harness) => harness.builtin)?.title ?? '';
    return harnesses?.find((harness) => harness.id === id)?.title ?? id;
}

export function WorkFoldersPage() {
    const panel = usePanel();
    const { folders, problem, reread } = useWorkFolders();
    const agents = useHarnesses();
    const [adding, setAdding] = useState(false);
    return (
        <div class="wk-page">
            <button type="button" class="deeper-back" onClick={() => panel.go('works', '')}>
                <IconChevronLeft size={16} />
                {tr.router.foldersBack}
            </button>
            <section class="card glass card-list" aria-labelledby="wf-folders-h">
                <div class="card-head">
                    <h2 id="wf-folders-h" class="t-title-xs m-0">
                        {tr.router.foldersTitle}
                    </h2>
                    {!adding && folders && (
                        <button type="button" class="pill pill-own" onClick={() => setAdding(true)}>
                            {tr.router.folderAdd}
                        </button>
                    )}
                </div>
                <p class="w-cap m-0 wk-under">{tr.router.foldersAbout}</p>
                {problem && (
                    <p class="w-cap m-0 c-dot-offline" role="alert">
                        {problem}
                    </p>
                )}
                {adding && (
                    <FolderForm
                        harnesses={agents.harnesses}
                        onCancel={() => setAdding(false)}
                        onSaved={() => {
                            setAdding(false);
                            reread();
                        }}
                    />
                )}
                {folders && folders.length === 0 && !adding && <p class="w-cap m-0 wk-empty">{tr.router.foldersEmpty}</p>}
                {folders && folders.length > 0 && (
                    <ul class="wk-rows">
                        {folders.map((folder) => (
                            <FolderRow key={folder.id} folder={folder} harnesses={agents.harnesses} onChanged={reread} />
                        ))}
                    </ul>
                )}
            </section>
            <Agents {...agents} />
        </div>
    );
}

function FolderRow({ folder, harnesses, onChanged }: { folder: WorkFolderView; harnesses: HarnessView[] | null; onChanged: () => void }) {
    const [editing, setEditing] = useState(false);
    const [problem, setProblem] = useState<string | null>(null);
    const remove = async () => {
        const sure = await ask({ title: tr.router.folderRemoveTitle, text: tr.router.folderRemoveText(folder.name), ok: tr.router.folderRemove, danger: true });
        if (!sure) return;
        api.removeWorkFolder(folder.id)
            .then(onChanged)
            .catch((e) => setProblem(problemOf(e)));
    };
    const facts = [
        folder.git ? (folder.branch ? tr.router.folderBranchNow(folder.branch) : null) : tr.router.folderNoGit,
        `${tr.router.folderAgent}: ${agentTitle(harnesses, folder.agent)}${folder.model ? ` · ${folder.model}` : ''}`,
        folder.maxUsd ? tr.router.folderCeiling(String(folder.maxUsd)) : null,
    ].filter(Boolean);
    return (
        <li class="wk-row wf-folder">
            <div class="wf-folder-head">
                <span class={`wk-dot ${folder.git ? 'is-own' : 'is-failed'}`} aria-hidden="true" />
                <div class="wk-row-main">
                    <span class="wk-row-title">
                        <b>{folder.name}</b>
                    </span>
                    <code class="wk-code wf-path">{folder.path}</code>
                    <span class="wk-meta">{facts.join(' · ')}</span>
                    {folder.allow.some((rule) => rule.includes('git push')) && <span class="wk-meta">{tr.router.folderPushOn}</span>}
                </div>
                {!editing && (
                    <div class="wk-actions">
                        <button type="button" class="pill" onClick={() => setEditing(true)}>
                            {tr.router.folderEdit}
                        </button>
                        <button type="button" class="pill" onClick={() => void remove()}>
                            {tr.router.folderRemove}
                        </button>
                    </div>
                )}
            </div>
            {problem && (
                <p class="w-cap m-0 c-dot-offline" role="alert">
                    {problem}
                </p>
            )}
            {editing && (
                <FolderForm
                    folder={folder}
                    harnesses={harnesses}
                    onCancel={() => setEditing(false)}
                    onSaved={() => {
                        setEditing(false);
                        onChanged();
                    }}
                />
            )}
        </li>
    );
}

/** Новая папка или правка папки: путь — только у новой, остальное — у обеих. */
function FolderForm({
    folder,
    harnesses,
    onCancel,
    onSaved,
}: {
    folder?: WorkFolderView;
    harnesses: HarnessView[] | null;
    onCancel: () => void;
    onSaved: () => void;
}) {
    const [name, setName] = useState(folder?.name ?? '');
    const [path, setPath] = useState('');
    const [agent, setAgent] = useState(folder?.agent ?? '');
    const [model, setModel] = useState(folder?.model ?? '');
    const [maxUsd, setMaxUsd] = useState(folder?.maxUsd ? String(folder.maxUsd) : '');
    const [prepare, setPrepare] = useState((folder?.prepare ?? []).join('\n'));
    const [checks, setChecks] = useState((folder?.checks ?? []).join('\n'));
    const [allow, setAllow] = useState((folder?.allow ?? []).join('\n'));
    const [busy, setBusy] = useState(false);
    const [problem, setProblem] = useState<string | null>(null);
    const save = () => {
        const usd = maxUsd.trim() === '' ? 0 : Number(maxUsd.replace(',', '.'));
        const input: WorkFolderInput = {
            name: name.trim() || undefined,
            agent,
            model,
            maxUsd: Number.isFinite(usd) ? usd : -1,
            prepare: lines(prepare),
            checks: lines(checks),
            allow: lines(allow),
        };
        if (!folder) input.path = path.trim();
        setBusy(true);
        setProblem(null);
        (folder ? api.updateWorkFolder(folder.id, input) : api.addWorkFolder(input))
            .then(onSaved)
            .catch((e) => setProblem(problemOf(e)))
            .finally(() => setBusy(false));
    };
    const area = (label: string, hint: string, value: string, set: (value: string) => void) => (
        <label class="flex flex-col gap-4">
            <span class="t-caption">{label}</span>
            <textarea class="field wf-lines" rows={2} spellcheck={false} value={value} onInput={(e) => set((e.currentTarget as HTMLTextAreaElement).value)} />
            <span class="t-meta">{hint}</span>
        </label>
    );
    return (
        <form
            class="wf-form"
            onSubmit={(event) => {
                event.preventDefault();
                save();
            }}
        >
            {!folder && (
                <label class="flex flex-col gap-4">
                    <span class="t-caption">{tr.router.folderPath}</span>
                    <input class="field" autocomplete="off" spellcheck={false} value={path} autoFocus onInput={(e) => setPath((e.target as HTMLInputElement).value)} />
                    <span class="t-meta">{tr.router.folderPathHint}</span>
                </label>
            )}
            <label class="flex flex-col gap-4">
                <span class="t-caption">{tr.router.folderName}</span>
                <input class="field" autocomplete="off" value={name} placeholder={tr.router.folderNameHint} onInput={(e) => setName((e.target as HTMLInputElement).value)} />
            </label>
            <div class="wf-pair">
                <label class="flex flex-col gap-4">
                    <span class="t-caption">{tr.router.folderAgent}</span>
                    <select class="field" value={agent} onChange={(e) => setAgent((e.target as HTMLSelectElement).value)}>
                        <option value="">{`${tr.router.folderAgentSeat} — ${agentTitle(harnesses, undefined)}`}</option>
                        {(harnesses ?? []).map((harness) => (
                            <option key={harness.id} value={harness.id}>
                                {harness.title}
                            </option>
                        ))}
                    </select>
                </label>
                <label class="flex flex-col gap-4">
                    <span class="t-caption">{tr.router.folderModel}</span>
                    <input class="field" autocomplete="off" spellcheck={false} value={model} placeholder={tr.router.folderModelHint} onInput={(e) => setModel((e.target as HTMLInputElement).value)} />
                </label>
            </div>
            <label class="flex flex-col gap-4 wf-usd">
                <span class="t-caption">{tr.router.folderMaxUsd}</span>
                <input class="field" inputMode="decimal" autocomplete="off" value={maxUsd} placeholder={tr.router.folderMaxUsdHint} onInput={(e) => setMaxUsd((e.target as HTMLInputElement).value)} />
            </label>
            {area(tr.router.folderPrepare, tr.router.folderPrepareHint, prepare, setPrepare)}
            {area(tr.router.folderChecks, tr.router.folderChecksHint, checks, setChecks)}
            {area(tr.router.folderAllow, tr.router.folderAllowHint, allow, setAllow)}
            {allow.includes('git push') && <p class="t-meta m-0 wf-warn">{tr.router.folderPushOn}</p>}
            {harnesses?.find((harness) => harness.id === agent)?.card?.asks === false && <p class="t-meta m-0 wf-warn">{tr.router.agentNoQuestions}</p>}
            <p class="t-meta m-0 wf-warn">{tr.router.folderRisk}</p>
            {problem && (
                <p class="w-cap m-0 c-dot-offline" role="alert">
                    {problem}
                </p>
            )}
            <div class="wk-actions">
                <button type="submit" class="pill pill-own" disabled={busy || (!folder && !path.trim())}>
                    {busy ? (folder ? tr.router.folderSaving : tr.router.folderAdding) : folder ? tr.router.folderSave : tr.router.folderAdd}
                </button>
                <button type="button" class="pill" disabled={busy} onClick={onCancel}>
                    {tr.router.folderCancel}
                </button>
            </div>
        </form>
    );
}

// ─── «Агенты» ───────────────────────────────────────────────────────────────────────────────────────────────────

/** Готов ли агент — строками: где найден, версия, вход, чем платит; не готов — почему и что сделать. */
export function readyLines(harness: HarnessView): string[] {
    const ready = harness.ready;
    const out: string[] = [];
    const not = ready.not;
    if (not?.why === 'missing') return [tr.router.agentMissing];
    if (ready.path) out.push(tr.router.agentFound(ready.path));
    if (not?.why === 'old') out.push(tr.router.agentOld(not.version ?? '?', not.need ?? '?'));
    else if (not?.why === 'unknown') out.push(not.detail ? tr.router.agentProtocol(not.detail) : tr.router.agentUnknown);
    else if (ready.version) out.push(tr.router.agentVersion(ready.version));
    if (not?.why === 'no-key') out.push(tr.router.agentNoKey);
    else if (ready.signedIn === true) out.push(ready.pays === 'key' ? `${tr.router.agentSignedIn} · ${tr.router.agentPaysKey}` : ready.pays === 'plan' ? `${tr.router.agentSignedIn} · ${tr.router.agentPaysPlan}` : tr.router.agentSignedIn);
    else if (ready.signedIn === null && !harness.builtin && !not) out.push(tr.router.agentSignInHere(harness.title));
    else if (ready.signedIn === false) out.push(tr.router.agentSignedOut(harness.title));
    else if (!not && ready.version) out.push(tr.router.agentSignedUnknown);
    return out;
}

function Agents({ harnesses, problem, set }: { harnesses: HarnessView[] | null; problem: string | null; reread: () => void; set: (fresh: HarnessView) => void }) {
    return (
        <section class="card glass card-list" aria-labelledby="wf-agents-h">
            <div class="card-head">
                <h2 id="wf-agents-h" class="t-title-xs m-0">
                    {tr.router.agentsTitle}
                </h2>
            </div>
            <p class="w-cap m-0 wk-under">{tr.router.agentsAbout}</p>
            {problem && (
                <p class="w-cap m-0 c-dot-offline" role="alert">
                    {problem}
                </p>
            )}
            {!harnesses && !problem && <p class="w-cap m-0">{tr.router.reading}</p>}
            {harnesses && (
                <ul class="wk-rows">
                    {harnesses.map((harness) => (
                        <AgentRow key={harness.id} harness={harness} onChanged={set} />
                    ))}
                </ul>
            )}
            <p class="w-cap m-0 wk-under">{tr.router.agentsMore}</p>
        </section>
    );
}

function AgentRow({ harness, onChanged }: { harness: HarnessView; onChanged: (fresh: HarnessView) => void }) {
    const ok = !harness.ready.not;
    const install = harness.ready.not?.why === 'missing' ? harness.ready.not.install : undefined;
    return (
        <li class="wk-row wf-agent">
            <div class="wf-folder-head">
                <span class={`wk-dot ${ok ? 'is-own' : 'is-failed'}`} aria-hidden="true" />
                <div class="wk-row-main">
                    <span class="wk-row-title">
                        <b>{harness.title}</b>
                        <span class="wk-kind">{[harness.builtin ? tr.router.agentBuiltin : null, ok ? tr.router.agentReady : tr.router.agentNotReady].filter(Boolean).join(' · ')}</span>
                    </span>
                    {readyLines(harness).map((line) => (
                        <span key={line} class="wk-meta">
                            {line}
                        </span>
                    ))}
                    {install && (
                        <>
                            <span class="wk-meta">{tr.router.agentInstall}</span>
                            <code class="wk-code wf-path">{install}</code>
                        </>
                    )}
                </div>
            </div>
            {harness.builtin ? <ClaudeSettings harness={harness} onChanged={onChanged} /> : <CardAgent harness={harness} onChanged={onChanged} />}
        </li>
    );
}

/**
 * Агент карточки из MagicPlace: где его программа (пусто — по имени), его условия; агент без вопросов перед командами
 * помечен — разрешённые команды папки он не спрашивает.
 */
function CardAgent({ harness, onChanged }: { harness: HarnessView; onChanged: (fresh: HarnessView) => void }) {
    const [path, setPath] = useState(harness.pathSet ?? '');
    const [busy, setBusy] = useState(false);
    const [said, setSaid] = useState<{ text: string; failed: boolean } | null>(null);
    const card = harness.card;
    const save = () => {
        setBusy(true);
        setSaid(null);
        api.setAgentPath(harness.id, path.trim())
            .then((fresh) => {
                onChanged(fresh);
                setSaid({ text: tr.router.folderSaved, failed: false });
            })
            .catch((e) => setSaid({ text: problemOf(e), failed: true }))
            .finally(() => setBusy(false));
    };
    return (
        <div class="wf-claude">
            {harness.description && <p class="t-meta m-0">{harness.description}</p>}
            {card?.asks === false && <p class="t-meta m-0 wf-warn">{tr.router.agentNoQuestions}</p>}
            {harness.ready.not?.why === 'signed-out' && harness.ready.not.hint && <p class="t-meta m-0">{harness.ready.not.hint}</p>}
            <label class="flex flex-col gap-4">
                <span class="t-caption">{tr.router.pathLabel}</span>
                <div class="wf-path-row">
                    <input class="field" autocomplete="off" spellcheck={false} placeholder={card?.program.name ?? tr.router.pathHint} value={path} onInput={(e) => setPath((e.target as HTMLInputElement).value)} />
                    <button type="button" class="pill" disabled={busy || path === (harness.pathSet ?? '')} onClick={save}>
                        {tr.router.pathSave}
                    </button>
                </div>
            </label>
            {card?.terms && (
                <a class="t-caption c-accent-text" href={card.terms} target="_blank" rel="noopener noreferrer">
                    {tr.router.agentTerms(harness.title)}
                </a>
            )}
            {said && (
                <p class={`w-cap m-0${said.failed ? ' c-dot-offline' : ''}`} role={said.failed ? 'alert' : 'status'}>
                    {said.text}
                </p>
            )}
        </div>
    );
}

/** Чем платит встроенный агент и где его программа. */
function ClaudeSettings({ harness, onChanged }: { harness: HarnessView; onChanged: (fresh: HarnessView) => void }) {
    const [busy, setBusy] = useState(false);
    const [said, setSaid] = useState<{ text: string; failed: boolean } | null>(null);
    const [key, setKey] = useState('');
    const [path, setPath] = useState(harness.pathSet ?? '');
    const [providers, setProviders] = useState<ProviderView[]>([]);
    const auth = harness.auth ?? 'terminal';
    useEffect(() => {
        let alive = true;
        api.brain()
            .then((brain) => alive && setProviders(brain.providers.filter((provider) => provider.kind === 'anthropic' && provider.key.set)))
            .catch(() => undefined);
        return () => {
            alive = false;
        };
    }, []);
    const change = (body: Parameters<typeof api.setClaudeCode>[0], done?: () => void) => {
        setBusy(true);
        setSaid(null);
        api.setClaudeCode(body)
            .then((fresh) => {
                onChanged(fresh);
                setSaid({ text: tr.router.folderSaved, failed: false });
                done?.();
            })
            .catch((e) => setSaid({ text: problemOf(e), failed: true }))
            .finally(() => setBusy(false));
    };
    return (
        <div class="wf-claude">
            <fieldset class="wf-auth">
                <legend class="t-caption">{tr.router.authTitle(harness.title)}</legend>
                {!harness.auth && harness.ready.signedIn === false && <p class="t-meta m-0">{tr.router.authChoose}</p>}
                <label class="wf-radio">
                    <input type="radio" name="claude-auth" checked={auth === 'terminal'} disabled={busy} onChange={() => change({ auth: 'terminal' })} />
                    <span>
                        <b>{tr.router.authTerminal}</b>
                        <span class="t-meta">{tr.router.authTerminalSub(harness.title)}</span>
                    </span>
                </label>
                <label class="wf-radio">
                    <input type="radio" name="claude-auth" checked={auth === 'key'} disabled={busy} onChange={() => change({ auth: 'key' })} />
                    <span>
                        <b>{tr.router.authKey}</b>
                        <span class="t-meta">{tr.router.authKeySub}</span>
                    </span>
                </label>
            </fieldset>
            {auth === 'key' && (
                <div class="flex flex-col gap-8">
                    <label class="flex flex-col gap-4">
                        <span class="t-caption">
                            {tr.router.keyLabel}
                            {harness.keySet ? ` · ${tr.router.keySet}` : ''}
                        </span>
                        <input class="field" type="password" autocomplete="off" placeholder={tr.router.keyPlaceholder} value={key} onInput={(e) => setKey((e.target as HTMLInputElement).value)} />
                    </label>
                    <div class="wk-actions">
                        <button type="button" class="pill pill-own" disabled={busy || !key.trim()} onClick={() => change({ key: key.trim() }, () => setKey(''))}>
                            {tr.router.keySave}
                        </button>
                        {providers.map((provider) => (
                            <button key={provider.id} type="button" class="pill" disabled={busy} onClick={() => change({ keyFrom: provider.id })}>
                                {`${tr.router.keyFromBrain}: ${provider.title}`}
                            </button>
                        ))}
                    </div>
                </div>
            )}
            <label class="flex flex-col gap-4">
                <span class="t-caption">{tr.router.pathLabel}</span>
                <div class="wf-path-row">
                    <input class="field" autocomplete="off" spellcheck={false} placeholder={tr.router.pathHint} value={path} onInput={(e) => setPath((e.target as HTMLInputElement).value)} />
                    <button type="button" class="pill" disabled={busy || path === (harness.pathSet ?? '')} onClick={() => change({ path })}>
                        {tr.router.pathSave}
                    </button>
                </div>
            </label>
            {said && (
                <p class={`w-cap m-0${said.failed ? ' c-dot-offline' : ''}`} role={said.failed ? 'alert' : 'status'}>
                    {said.text}
                </p>
            )}
        </div>
    );
}
