// «Рабочие папки» (docs/plans/router-and-works.md, «Устройство: пульт», этап 3) — `#/works/folders`, со страницы
// «Работы» (решение владельца 16.11.2026: папки и работы — в одном месте). Папки на этом компьютере, где агент делает
// работы: добавить (только папка с git), агент и модель папки, потолок запуска, подготовка, проверки, разрешённые
// команды; убрать из списка. Рядом — «Агенты»: Claude Code и поставленные карточки — найден ли, версия, вошёл ли; у
// Claude Code — чем платит: «Как в терминале» или «По ключу API». Свою возможность именем Claude Code не называем:
// место — «Агенты», а Claude Code — один из них, обычным текстом.

import { useEffect, useState } from 'preact/hooks';

import {
    ApiError,
    AssignMode,
    ClaudePluginView,
    HarnessView,
    ModelSet,
    SetsView,
    ProviderView,
    WorkFolderInput,
    WorkFolderView,
    WorkSystemView,
    WorkSystemsSetup,
    api,
    problemOf,
} from '../api';
import { IconChevronLeft } from '../icons';
import { tr } from '../i18n';
import { usePoll } from '../poll';
import { usePanel } from '../shell/context';
import { CopyButton } from './work';
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

/** Наша система работ: её id у шлюза. */
const OURS = 'duowave';

/** Наборы моделей для работ (docs/plans/work-systems.md, этап 5); null — ещё не прочитаны или их нет у шлюза. */
export function useWorkSets(): SetsView | null {
    const [sets, setSets] = useState<SetsView | null>(null);
    useEffect(() => {
        const abort = new AbortController();
        api.workSets(abort.signal)
            .then(setSets)
            .catch(() => undefined);
        return () => abort.abort();
    }, []);
    return sets;
}

/** Название набора: три с DuoWave на языке пульта, свой по его названию; нет набора — «без набора». */
export function setTitle(sets: SetsView, id: string | undefined): string {
    if (!id) return tr.router.setNone;
    if (id === 'economy') return tr.router.setEconomy;
    if (id === 'usual') return tr.router.setUsual;
    if (id === 'quality') return tr.router.setQuality;
    return sets.sets.find((set) => set.id === id)?.title ?? id;
}

/** Что набор даёт агентам: «Claude Code: sonnet». */
function setModels(set: ModelSet, harnesses: HarnessView[] | null): string {
    return Object.entries(set.models)
        .map(([agent, tiers]) => `${agentTitle(harnesses, agent)}: ${tiers.mid ?? tiers.high ?? tiers.low ?? ''}`)
        .join(', ');
}

/**
 * Наборы моделей (этап 5): набор решает, на какой модели агент делает работу. Набор дома — здесь; папка и фраза
 * («сделай подешевле …») выбирают свой. Свои наборы — через API (`PUT /api/v1/work-sets`).
 */
function Sets({ harnesses }: { harnesses: HarnessView[] | null }) {
    const [sets, setSets] = useState<SetsView | null>(null);
    const [problem, setProblem] = useState<string | null>(null);
    useEffect(() => {
        const abort = new AbortController();
        api.workSets(abort.signal)
            .then(setSets)
            .catch((e) => {
                if (!abort.signal.aborted) setProblem(problemOf(e));
            });
        return () => abort.abort();
    }, []);
    if (!sets && !problem) return null;
    const choose = (id: string) => {
        if (!sets) return;
        setProblem(null);
        api.setWorkSets({ default: id || undefined, own: sets.own })
            .then(setSets)
            .catch((e) => setProblem(problemOf(e)));
    };
    return (
        <section class="card glass card-list" aria-labelledby="wf-sets-h">
            <div class="card-head">
                <h2 id="wf-sets-h" class="t-title-xs m-0">
                    {tr.router.setsTitle}
                </h2>
            </div>
            <p class="w-cap m-0 wk-under">{tr.router.setsAbout}</p>
            {problem && (
                <p class="w-cap m-0 c-dot-offline" role="alert">
                    {problem}
                </p>
            )}
            {sets && (
                <>
                    <label class="flex flex-col gap-4 wk-under">
                        <span class="w-cap">{tr.router.setsDefault}</span>
                        <select class="field" value={sets.default ?? ''} onChange={(e) => choose((e.target as HTMLSelectElement).value)}>
                            <option value="">{tr.router.setNone}</option>
                            {sets.sets.map((set) => (
                                <option key={set.id} value={set.id}>
                                    {setTitle(sets, set.id)}
                                </option>
                            ))}
                        </select>
                    </label>
                    <ul class="wk-rows">
                        {sets.sets.map((set) => (
                            <li key={set.id} class="wk-row">
                                <div class="wk-row-main">
                                    <span class="wk-row-title">
                                        <b>{setTitle(sets, set.id)}</b>
                                    </span>
                                    <span class="wk-meta">{setModels(set, harnesses)}</span>
                                </div>
                            </li>
                        ))}
                    </ul>
                </>
            )}
        </section>
    );
}

/** Системы работ, что начинают работы, кроме нашей, и кому что поручать; null — ещё не прочитаны. */
function useWorkSystems(): {
    systems: WorkSystemView[] | null;
    setup: WorkSystemsSetup | null;
    problem: string | null;
    setSetup: (fresh: WorkSystemsSetup) => void;
} {
    const [systems, setSystems] = useState<WorkSystemView[] | null>(null);
    const [setup, setSetup] = useState<WorkSystemsSetup | null>(null);
    const [problem, setProblem] = useState<string | null>(null);
    useEffect(() => {
        const abort = new AbortController();
        Promise.all([api.workSystems(abort.signal), api.workSystemsSetup(abort.signal)])
            .then(([list, fresh]) => {
                setSystems(list.systems.filter((system) => system.id !== OURS && system.can.start));
                setSetup(fresh);
            })
            .catch((e) => {
                if (!abort.signal.aborted) setProblem(problemOf(e));
            });
        return () => abort.abort();
    }, []);
    return { systems, setup, problem, setSetup };
}

/** Ответ на «Что поручать …?» по тому, как всё настроено сейчас. */
function modeOf(setup: WorkSystemsSetup | null, id: string): AssignMode {
    if (setup?.default === id) return 'all';
    if (Object.values(setup?.folders ?? {}).includes(id)) return 'places';
    return 'named';
}

/** Кто делает работы папки: её своя система, иначе система дома, иначе DuoWave. */
function folderSystem(setup: WorkSystemsSetup | null, folder: string): string {
    return setup?.folders?.[folder] ?? setup?.default ?? OURS;
}

export function WorkFoldersPage() {
    const panel = usePanel();
    const { folders, problem, reread } = useWorkFolders();
    const agents = useHarnesses();
    const systems = useWorkSystems();
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
                            <FolderRow key={folder.id} folder={folder} harnesses={agents.harnesses} systems={systems} onChanged={reread} />
                        ))}
                    </ul>
                )}
            </section>
            <Systems {...systems} />
            <Sets harnesses={agents.harnesses} />
            <Agents {...agents} />
            <ByHand />
        </div>
    );
}

/** Что система умеет, словами: начать, показать свои работы, принять слова вдогонку, остановить, открыть у себя. */
function canLine(system: WorkSystemView): string {
    const can = system.can;
    const verbs = [
        can.start ? tr.router.canStart : null,
        can.list ? tr.router.canList : null,
        can.tellNow || can.tellAfter ? tr.router.canTell : null,
        can.stop ? tr.router.canStop : null,
        can.open ? tr.router.canOpen : null,
        can.places ? tr.router.canPlaces : null,
    ].filter(Boolean);
    return tr.router.systemCan(verbs.join(', '));
}

/**
 * Системы работ (docs/plans/work-systems.md, этап 4): кроме DuoWave работы делают bb, T3 Code и системы карточек
 * MagicPlace. У каждой: готова ли, что умеет, и вопрос «Что поручать …?» — все работы, работы в её проектах или только
 * когда назову. Claude Code в терминале работ не начинает: он ниже, своей карточкой.
 */
function Systems({
    systems,
    setup,
    problem,
    setSetup,
}: {
    systems: WorkSystemView[] | null;
    setup: WorkSystemsSetup | null;
    problem: string | null;
    setSetup: (fresh: WorkSystemsSetup) => void;
}) {
    const [failed, setFailed] = useState<string | null>(null);
    if (!problem && (!systems || systems.length === 0)) return null;
    const assign = (id: string, mode: AssignMode) => {
        setFailed(null);
        api.assignWorkSystem(id, mode)
            .then(setSetup)
            .catch((e) => setFailed(problemOf(e)));
    };
    const shown = problem ?? failed;
    return (
        <section class="card glass card-list" aria-labelledby="wf-systems-h">
            <div class="card-head">
                <h2 id="wf-systems-h" class="t-title-xs m-0">
                    {tr.router.systemsTitle}
                </h2>
            </div>
            <p class="w-cap m-0 wk-under">{tr.router.systemsAbout}</p>
            {shown && (
                <p class="w-cap m-0 c-dot-offline" role="alert">
                    {shown}
                </p>
            )}
            {systems && systems.length > 0 && (
                <ul class="wk-rows">
                    {systems.map((system) => (
                        <li key={system.id} class="wk-row wf-folder">
                            <div class="wf-folder-head">
                                <span class={`wk-dot ${system.ready.ready ? 'is-own' : 'is-failed'}`} aria-hidden="true" />
                                <div class="wk-row-main">
                                    <span class="wk-row-title">
                                        <b>{system.title}</b>
                                    </span>
                                    <span class="wk-meta">
                                        {system.ready.ready ? tr.router.systemReady : tr.router.systemNotReady(system.ready.why ?? '')}
                                    </span>
                                    <span class="wk-meta">{canLine(system)}</span>
                                </div>
                            </div>
                            <label class="flex flex-col gap-4 wk-under">
                                <span class="w-cap">{tr.router.assignQuestion(system.title)}</span>
                                <select
                                    class="field"
                                    value={modeOf(setup, system.id)}
                                    onChange={(e) => assign(system.id, (e.target as HTMLSelectElement).value as AssignMode)}
                                >
                                    <option value="all">{tr.router.assignAll}</option>
                                    {system.can.places && <option value="places">{tr.router.assignPlaces}</option>}
                                    <option value="named">{tr.router.assignNamed}</option>
                                </select>
                            </label>
                        </li>
                    ))}
                </ul>
            )}
        </section>
    );
}

/**
 * Claude Code, запущенный вами в терминале (docs/plans/work-systems.md, этап 3): DuoWave видит его сессии и так. Плагин
 * DuoWave для Claude Code приносит остальное: вопросы о разрешении голосом, конец хода, слова вдогонку. Ставите его вы
 * сами, двумя командами; убираете так же.
 */
function ByHand() {
    const [plugin, setPlugin] = useState<ClaudePluginView | null>(null);
    const [problem, setProblem] = useState<string | null>(null);
    const [busy, setBusy] = useState(false);
    const show = () => {
        setBusy(true);
        setProblem(null);
        api.claudePlugin()
            .then(setPlugin)
            .catch((e) => setProblem(problemOf(e)))
            .finally(() => setBusy(false));
    };
    const commands = (title: string, lines: string[]) => (
        <div class="wk-under">
            <p class="w-cap m-0">{title}</p>
            <pre class="wk-code m-0">{lines.join('\n')}</pre>
            <CopyButton text={lines.join('\n')} />
        </div>
    );
    return (
        <section class="card glass card-list" aria-labelledby="wf-byhand-h">
            <div class="card-head">
                <h2 id="wf-byhand-h" class="t-title-xs m-0">
                    {tr.router.byHandTitle}
                </h2>
                {!plugin && (
                    <button type="button" class="pill pill-own" disabled={busy} onClick={show}>
                        {busy ? '…' : tr.router.byHandShow}
                    </button>
                )}
            </div>
            <p class="w-cap m-0 wk-under">{tr.router.byHandAbout}</p>
            {problem && (
                <p class="w-cap m-0 c-dot-offline" role="alert">
                    {problem}
                </p>
            )}
            {plugin && commands(tr.router.byHandInstall, plugin.install)}
            {plugin && commands(tr.router.byHandRemove, plugin.remove)}
        </section>
    );
}

function FolderRow({
    folder,
    harnesses,
    systems,
    onChanged,
}: {
    folder: WorkFolderView;
    harnesses: HarnessView[] | null;
    systems: ReturnType<typeof useWorkSystems>;
    onChanged: () => void;
}) {
    const [editing, setEditing] = useState(false);
    const [problem, setProblem] = useState<string | null>(null);
    // «Кто делает»: своя система папки — когда кроме DuoWave есть другие.
    const others = systems.systems ?? [];
    const who = folderSystem(systems.setup, folder.id);
    const setWho = (id: string) => {
        setProblem(null);
        const folders = { ...(systems.setup?.folders ?? {}), [folder.id]: id };
        api.setWorkSystemsSetup({ ...(systems.setup ?? {}), folders })
            .then(systems.setSetup)
            .catch((e) => setProblem(problemOf(e)));
    };
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
                    {others.length > 0 && (
                        <label class="flex items-center gap-8 wk-meta">
                            <span>{tr.router.folderSystem}</span>
                            <select class="field" value={who} onChange={(e) => setWho((e.target as HTMLSelectElement).value)}>
                                <option value={OURS}>DuoWave</option>
                                {others.map((system) => (
                                    <option key={system.id} value={system.id}>
                                        {system.title}
                                    </option>
                                ))}
                            </select>
                        </label>
                    )}
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
    const [set, setSet] = useState(folder?.set ?? '');
    const sets = useWorkSets();
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
            set,
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
            {sets && (
                <label class="flex flex-col gap-4">
                    <span class="t-caption">{tr.router.folderSet}</span>
                    <select class="field" value={set} onChange={(e) => setSet((e.target as HTMLSelectElement).value)}>
                        <option value="">{tr.router.setOfHome(setTitle(sets, sets.default))}</option>
                        {sets.sets.map((entry) => (
                            <option key={entry.id} value={entry.id}>
                                {setTitle(sets, entry.id)}
                            </option>
                        ))}
                    </select>
                </label>
            )}
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
