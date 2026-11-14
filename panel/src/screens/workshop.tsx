// «Мастерская» («Плагины» → «Мастерская»; docs/plans/plugins.md, этап 6): плагин или тема пишется в черновике —
// агентом в чате или руками —, проверяется в песочнице шлюза и ставится новой версией рядом с прежней. «Поставить»
// всегда сначала показывает, что изменится (версия, права, хосты, что даёт), и ставится только по «да» администратора:
// агент и голос этого не могут. Плагин можно попробовать на колонке, не ставя насовсем: проба идёт на шлюзе несколько
// минут, потом прежняя версия возвращается сама. Тему — примерить в пульте. Версии на шлюзе и откат — здесь же.
// «Ход работы» показывает, что агент делает с черновиком сейчас и делал раньше — и по просьбе голосом у колонки.
// Поставленный виджет стола встаёт и на стол.

import { useEffect, useMemo, useRef, useState } from 'preact/hooks';

import {
    AgentJob,
    AgentStep,
    BrainView,
    CheckReport,
    DraftView,
    InstallPlan,
    InstallReport,
    JournalLine,
    PackageView,
    PhraseReport,
    RunReport,
    WorkshopAgentModel,
    WorkshopEvent,
    api,
    can,
    problemOf,
    sessionStore,
    workshopChat,
} from '../api';
import { IconCheck } from '../icons';
import { syncPackages } from '../plugins/loader';
import { forgetStationPackages } from '../sdk';
import { POLL_MS, aboutOf, askToStop, doingWords, elapsed, shortText, toolWords, useSecondTick, useSettledVersion } from '../shell/agent-plaque';
import { useLinkedItem, usePanel } from '../shell/context';
import { mayLeave, useLeaveGuard } from '../shell/leave';
import { PanelPlugin, onRegistryChange, registerPlugin, registeredPlugins, unregisterPlugin } from '../shell/registry';
import { setTheme, useTheme } from '../shell/theme';
import { Theme, normalizeTheme } from '../shell/themes';
import { deskActions } from '../shell/widgets';
import { timeAgo } from '../time';
import { Action } from '../ui/cards';
import { Sheet } from '../ui/sheet';
import { locale, tr } from '../i18n';
import { VIA } from '../via';
import { RightsLines } from '../ui/rights-lines';
import { ask as confirmAsk } from '../ui/confirm';
import { pushDraft, stationInstall, stationPlan, stationTrial, stationVersion } from './to-station';
import { DraftWidgetPreview, mayRunDraft } from './widget-agent';
import { DraftCardPreview } from '../cards/draft-card';
import { HistoryCard, HistoryRoomLine } from './history';
import { CodeEditor } from '../ui/code-editor';
import { byMode, device, examplePhrase } from '../words';

const PREVIEW = 'workshop.preview';
const isAdmin = (): boolean => can('plugins.manage') && sessionStore.get()?.role === 'admin';
const TRIAL_MINUTES = [5, 10, 30];

/** id черновика из чего угодно: строчная латиница, цифры и «-», с буквы, до 32 знаков. */
const idOf = (text: string): string =>
    text
        .toLowerCase()
        .replace(/[^a-z0-9-]+/g, '-')
        .replace(/^[^a-z]+/, '')
        .replace(/-+/g, '-')
        .replace(/-$/, '')
        .slice(0, 32);

/** Тема пульта как данные манифеста: всё, что normalizeTheme прочтёт обратно. */
function themeData(id: string, title: string, base: Theme) {
    return {
        id,
        title,
        about: tr.workshop.themeAbout(base.title),
        modes: base.modes,
        modeNotes: base.modeNotes,
        defaultMode: base.defaultMode,
        look: 'minimal',
        layout: base.layout,
        density: base.density,
        font: base.font,
        radius: base.radius,
        palette: base.palette,
    };
}

/** «Научить» из «Пока не умеем»: задание агенту для черновика, который для этого сделали, — ждёт его чата. */
const teaching = new Map<string, string>();

/** Задание «Научить» для чата черновика: одно и один раз. */
function takeTeaching(draft: string): string {
    const task = teaching.get(draft) ?? '';
    teaching.delete(draft);
    return task;
}

/** Черновики и новый черновик. */
function DraftList({ drafts, onOpen, onMade }: { drafts: DraftView[]; onOpen: (id: string) => void; onMade: (draft: DraftView) => void }) {
    const linked = useLinkedItem('skills', 'workshop');
    const theme = useTheme();
    // «Вид пульта» → «Сделать свою тему»: форма темы сразу, на основе выбранной.
    const base = linked.query.new === 'theme' ? (theme.themes.find((entry) => entry.id === linked.query.base) ?? theme.theme) : null;
    // «Навыки» → «Пока не умеем» → «Научить»: форма навыка сразу, с фразой в имени; задание — агенту.
    const teach = linked.query.new === 'plugin' ? linked.query.teach?.trim() || null : null;
    const [making, setMaking] = useState<'plugin' | 'quiz' | 'theme' | 'openapi' | null>(base ? 'theme' : teach ? 'plugin' : null);
    const [id, setId] = useState(base ? idOf(`my-${base.id}`) : teach ? `skill-${Date.now().toString(36).slice(-5)}` : '');
    const [title, setTitle] = useState(base ? tr.workshop.myTheme(base.title) : teach ? tr.workshop.teachTitle(teach) : '');
    const [openapi, setOpenapi] = useState('');
    const [busy, setBusy] = useState(false);
    const [error, setError] = useState<string | null>(null);
    const make = async () => {
        setBusy(true);
        setError(null);
        try {
            const body = making === 'openapi' ? { id: id.trim(), title: title.trim() || undefined, openapi } : { id: id.trim(), kind: making ?? 'plugin', title: title.trim() || undefined };
            let draft = await api.createDraft(body as Parameters<typeof api.createDraft>[0]);
            if (making === 'theme' && base) {
                // Своя тема начинается с выбранной: её палитры обоих режимов, раскладка, плотность и скругления.
                const manifest = JSON.parse((await api.readDraftFile(draft.id, 'manifest.json')).content);
                manifest.contributes = { ...(manifest.contributes ?? {}), themes: [themeData(draft.id, title.trim() || draft.id, base)] };
                draft = await api.writeDraftFile(draft.id, 'manifest.json', `${JSON.stringify(manifest, null, 2)}\n`);
            }
            if (teach && making === 'plugin')
                teaching.set(
                    draft.id,
                    byMode(tr.workshop.teachTaskSpeaker, tr.workshop.teachTaskVoice)(teach),
                );
            onMade(draft);
            setMaking(null);
            setId('');
            setTitle('');
            setOpenapi('');
        } catch (e) {
            setError(problemOf(e));
        } finally {
            setBusy(false);
        }
    };
    return (
        <section class="card glass card-list" aria-labelledby="drafts-h">
            <div class="card-head">
                <h2 id="drafts-h" class="t-title-xs m-0">
                    {tr.workshop.drafts}
                </h2>
                <span class="t-meta">{drafts.length}</span>
            </div>
            <p class="t-caption m-0 mb-6">{tr.workshop.draftsAbout}</p>
            {drafts.length === 0 && <p class="t-caption m-0 mb-6">{tr.workshop.draftsEmpty}</p>}
            {drafts.map((draft) => (
                <div key={draft.id} class="pkg-row">
                    <div class="pkg-main">
                        <button class="pkg-text text-left cursor-pointer" onClick={() => onOpen(draft.id)}>
                            <span class="pkg-title">
                                {draft.title ?? draft.id}
                                <span class="pill pill-off">{draft.kind === 'theme' ? tr.workshop.kindTheme : tr.workshop.kindPlugin}</span>
                                {draft.trial && <span class="pill pill-agent">{tr.workshop.pillTrial}</span>}
                                {draft.problems.length > 0 && <span class="pill pill-error">{tr.workshop.manifestBroken}</span>}
                            </span>
                            <span class="t-caption">
                                {[draft.id, draft.version && tr.workshop.draftVersion(draft.version), draft.installed ? tr.workshop.installedVersion(draft.installed) : tr.workshop.notInstalledShort].filter(Boolean).join(' · ')}
                            </span>
                        </button>
                    </div>
                </div>
            ))}
            {making ? (
                <div class="ws-form">
                    <h3 class="t-title-xs m-0">
                        {making === 'theme'
                            ? base
                                ? tr.workshop.newThemeBased(base.title)
                                : tr.workshop.newTheme
                            : making === 'openapi'
                              ? tr.workshop.fromApiTitle
                              : making === 'quiz'
                                ? tr.workshop.quizTitle
                                : tr.workshop.newPlugin}
                    </h3>
                    <label class="ws-label">
                        <span class="t-caption">{tr.workshop.idLabel}</span>
                        <input class="field" value={id} onInput={(e) => setId((e.target as HTMLInputElement).value)} />
                    </label>
                    <label class="ws-label">
                        <span class="t-caption">{tr.workshop.nameLabel}</span>
                        <input class="field" value={title} onInput={(e) => setTitle((e.target as HTMLInputElement).value)} />
                    </label>
                    {making === 'openapi' && (
                        <label class="ws-label">
                            <span class="t-caption">{tr.workshop.openapiLabel}</span>
                            <textarea class="field font-mono ws-code" rows={8} spellcheck={false} value={openapi} onInput={(e) => setOpenapi((e.target as HTMLTextAreaElement).value)} />
                        </label>
                    )}
                    <div class="flex flex-wrap gap-8">
                        <Action tone="on" label={tr.workshop.create} busy={busy} disabled={!id.trim()} onClick={() => void make()} />
                        <Action label={tr.workshop.cancel} disabled={busy} onClick={() => (setMaking(null), base && linked.close())} />
                    </div>
                </div>
            ) : (
                <div class="flex flex-wrap gap-8 mt-8">
                    <Action tone="on" label={tr.workshop.newPlugin} onClick={() => setMaking('plugin')} />
                    <Action label={tr.workshop.quizButton} onClick={() => setMaking('quiz')} />
                    <Action label={tr.workshop.newThemeButton} onClick={() => setMaking('theme')} />
                    <Action label={tr.workshop.fromApiButton} onClick={() => setMaking('openapi')} />
                </div>
            )}
            {error && (
                <p class="t-caption m-0 c-dot-offline" role="alert">
                    {error}
                </p>
            )}
        </section>
    );
}

/** Итог проверки словами. */
function CheckLines({ report }: { report: CheckReport }) {
    return (
        <div class="flex flex-col gap-4">
            <span class={`t-caption ${report.ok ? 'c-accent-text' : 'c-dot-offline'}`}>
                {report.ok ? tr.workshop.checkOk(report.ms, report.exports.join(', ')) : tr.workshop.checkFailed}
            </span>
            {report.manifest.map((problem) => (
                <span key={problem} class="t-caption c-dot-offline">
                    {tr.workshop.manifestProblem(problem)}
                </span>
            ))}
            {report.code && <span class="t-caption c-dot-offline">{tr.workshop.codeProblem(report.code)}</span>}
            {report.missing.length > 0 && <span class="t-caption c-dot-offline">{tr.workshop.missingFunctions(report.missing.join(', '))}</span>}
            {report.warnings.map((warning) => (
                <span key={warning} class="t-caption c-pill-yandex-fg">
                    {warning}
                </span>
            ))}
            {(report.languages ?? []).map((hint) => (
                <span key={hint} class="t-caption">
                    {hint}
                </span>
            ))}
        </div>
    );
}

function LogLines({ lines }: { lines: JournalLine[] }) {
    if (lines.length === 0) return null;
    return (
        <ol class="ws-log t-caption">
            {lines.map((line, index) => (
                <li key={index} class={line.level === 'error' ? 'c-dot-offline' : line.level === 'warn' ? 'c-pill-yandex-fg' : ''}>
                    <b>{line.level}</b> {line.message}
                </li>
            ))}
        </ol>
    );
}

/** Права словами, с пометкой, что новое: новые — янтарём (их одобряют), ушедшие — зачёркнуты. */
/**
 * «Поставить версию X?» / «Проба на колонке»: что изменится — версия, права и хосты (новые выделены), что даёт, бюджет,
 * — и одно «да» администратора. Первая установка и новые права одобряются здесь же.
 */
function InstallSheet({
    plan,
    trial,
    busy,
    onInstall,
    onTrial,
    onClose,
}: {
    plan: InstallPlan;
    /** Открыто кнопкой пробы: главное действие — проба. */
    trial: boolean;
    busy: string | null;
    onInstall: () => void;
    onTrial: (minutes: number) => void;
    onClose: () => void;
}) {
    const [minutes, setMinutes] = useState(10);
    const plugin = plan.kind === 'plugin';
    const asks = plan.first || plan.added.length > 0 || plan.hostsAdded.length > 0;
    const title = trial ? tr.workshop.trialTitle(plan.to) : plan.first ? tr.workshop.installFirstTitle(plan.title, plan.to) : tr.workshop.installTitle(plan.to);
    const about = plan.first ? (plugin ? tr.workshop.aboutFirstPlugin : tr.workshop.aboutFirstTheme) : tr.workshop.aboutFromTo(plan.from ?? '', plan.to);
    const buttons = (
            <div class="flex flex-wrap gap-8">
                {trial ? (
                    <>
                        <Action tone="on" label={asks ? tr.workshop.approveTrial(minutes) : tr.workshop.tryMinutes(minutes)} busy={busy === 'trial'} disabled={!!busy} onClick={() => onTrial(minutes)} />
                        <Action label={tr.workshop.installForever(plan.to)} busy={busy === 'install'} disabled={!!busy} onClick={onInstall} />
                    </>
                ) : (
                    <>
                        <Action tone="on" label={asks ? tr.workshop.approveInstall(plan.to) : tr.workshop.installTo(plan.to)} busy={busy === 'install'} disabled={!!busy} onClick={onInstall} />
                        {plugin && <Action label={tr.workshop.trialMinutes(minutes)} busy={busy === 'trial'} disabled={!!busy} onClick={() => onTrial(minutes)} />}
                    </>
                )}
                <Action label={tr.workshop.cancel} disabled={!!busy} onClick={onClose} />
            </div>
    );
    return (
        <Sheet title={title} about={about} onClose={onClose} footer={buttons}>
            {trial && (
                <p class="t-body-sm m-0">
                    {plan.from
                        ? tr.workshop.trialBodyFrom(plan.from, byMode(tr.workshop.speakAsUsualSpeaker, tr.workshop.speakAsUsual))
                        : tr.workshop.trialBodyNew(byMode(tr.workshop.speakAsUsualSpeaker, tr.workshop.speakAsUsual))}
                </p>
            )}
            <RightsLines plan={plan} />
            {(plan.phrases.length > 0 || plan.tools.length > 0) && (
                <div class="flex flex-col gap-4">
                    <h3 class="t-title-xs m-0">{tr.workshop.gives}</h3>
                    {plan.phrases.length > 0 && (
                        <span class="t-caption">
                            {tr.workshop.phrasesLine(plan.phrases.slice(0, 6).map((phrase) => `«${phrase}»`).join(', '), plan.phrases.length > 6 ? tr.workshop.andMore(plan.phrases.length - 6) : '')}
                        </span>
                    )}
                    {plan.tools.length > 0 && <span class="t-caption">{tr.workshop.toolsLine(plan.tools.join(', '))}</span>}
                </div>
            )}
            {plugin && (
                <p class="t-caption m-0">
                    {tr.workshop.budget(plan.budget.memoryMb, plan.budget.storageMb)}
                </p>
            )}
            {!trial && plan.from && (
                <p class="t-caption m-0">
                    {tr.workshop.previousStays(plan.from, plan.keptVersions)}
                    {plugin && !plan.enabled ? tr.workshop.pluginDisabledNote : ''}
                </p>
            )}
            {plugin && (
                <div class="flex flex-col gap-6">
                    {!trial && <h3 class="t-title-xs m-0">{tr.workshop.orTryFirst}</h3>}
                    <div class="ws-chips" role="radiogroup" aria-label={tr.workshop.trialLengthLabel}>
                        <span class="t-caption">{byMode(tr.workshop.trialColonSpeaker, tr.workshop.trialColonVoice)}</span>
                        {TRIAL_MINUTES.map((value) => (
                            <button key={value} role="radio" aria-checked={minutes === value} class={`pill ${minutes === value ? 'pill-own' : 'pill-off'} cursor-pointer min-h-36`} onClick={() => setMinutes(value)}>
                                {tr.workshop.minShort(value)}
                            </button>
                        ))}
                    </div>
                </div>
            )}
        </Sheet>
    );
}

/** Сколько осталось до `until`, словами. */
function left(until: string, now: number): string {
    const minutes = Math.max(0, Math.ceil((Date.parse(until) - now) / 60000));
    return minutes <= 1 ? tr.workshop.lessThanMinute : tr.workshop.minShort(minutes);
}

/** Плашка пробы: какая версия работает, сколько осталось, «оставить» или «вернуть сейчас». */
function TrialBanner({ draft, busy, onEnd }: { draft: DraftView; busy: string | null; onEnd: (keep: boolean) => void }) {
    const trial = draft.trial!;
    const [now, setNow] = useState(Date.now());
    useEffect(() => {
        const timer = window.setInterval(() => setNow(Date.now()), 15000);
        return () => window.clearInterval(timer);
    }, []);
    return (
        <div class="ws-trial" role="status">
            <span class="t-body-sm">
                <b>{byMode(tr.workshop.trialColonSpeaker, tr.workshop.trialColonVoice)}</b>
                {tr.workshop.trialRunning(trial.version, trial.previous ?? undefined, left(trial.until, now))}
            </span>
            <div class="flex flex-wrap gap-8">
                <Action tone="on" label={tr.workshop.keepVersion} busy={busy === 'keep'} disabled={!!busy} onClick={() => onEnd(true)} />
                <Action label={trial.previous ? tr.workshop.returnNow(trial.previous) : tr.workshop.removeNow} busy={busy === 'end'} disabled={!!busy} onClick={() => onEnd(false)} />
            </div>
        </div>
    );
}

/** Версии на шлюзе: какая работает, какие прежние, откат к прошлой. */
function VersionsCard({ entry, busy, trial, onRollback }: { entry: PackageView; busy: string | null; trial: boolean; onRollback: () => void }) {
    const ready = entry.versions.filter((version) => version.problems.length === 0);
    const canRollback = (entry.removable ?? true) && ready.length > 1 && !trial;
    return (
        <section class="card glass card-list" aria-labelledby="versions-h">
            <div class="card-head">
                <h2 id="versions-h" class="t-title-xs m-0">
                    {tr.workshop.versionsTitle}
                </h2>
                <span class="t-meta">{entry.versions.length}</span>
            </div>
            <ol class="ws-versions">
                {entry.versions.map((version, index) => (
                    <li key={version.version} class="ws-version">
                        <span class="font-mono">{version.version}</span>
                        {version.version === entry.version && <span class={`pill ${entry.enabled ? 'pill-own' : 'pill-off'}`}>{entry.enabled ? tr.workshop.versionWorking : tr.workshop.versionChosenDisabled}</span>}
                        {index === 0 && version.version !== entry.version && <span class="pill pill-off">{tr.workshop.versionNewest}</span>}
                        {version.problems.length > 0 && <span class="t-caption c-dot-offline">{version.problems.join('; ')}</span>}
                    </li>
                ))}
            </ol>
            {canRollback && (
                <div class="mt-8">
                    <Action label={tr.workshop.rollbackTo(ready[1].version)} busy={busy === 'rollback'} disabled={!!busy} onClick={onRollback} />
                </div>
            )}
            {trial && <p class="t-caption m-0 mt-8">{tr.workshop.rollbackDuringTrial}</p>}
            {!trial && ready.length <= 1 && <p class="t-caption m-0 mt-8">{tr.workshop.nothingToRollback}</p>}
        </section>
    );
}

type ManifestTool = { name: string; handler?: string; description?: string; input?: { properties?: Record<string, { type?: string }> } };

/** Что из манифеста нужно песочнице: шаблоны фраз и инструменты с их входом. */
function readManifest(text: string): { phrases: string[]; tools: ManifestTool[] } {
    try {
        const manifest = JSON.parse(text);
        const intents: Array<{ phrases?: unknown[] }> = Array.isArray(manifest?.contributes?.intents) ? manifest.contributes.intents : [];
        const phrases = intents.flatMap((intent) => (Array.isArray(intent.phrases) ? intent.phrases.filter((phrase): phrase is string => typeof phrase === 'string') : []));
        const tools: ManifestTool[] = Array.isArray(manifest?.contributes?.tools) ? manifest.contributes.tools.filter((tool: ManifestTool) => typeof tool?.name === 'string') : [];
        return { phrases, tools };
    } catch {
        return { phrases: [], tools: [] };
    }
}

/** Фраза для прогона из шаблона: слот — его же именем («скажи привет {name}» → «скажи привет name»). */
const sample = (template: string): string => template.replace(/\{([^}]+)\}/g, '$1');

/** Пример входа по схеме инструмента: строка — "", число — 0, да/нет — false. */
function sampleInput(tool?: ManifestTool): string {
    const properties = tool?.input?.properties ?? {};
    const value = Object.fromEntries(Object.entries(properties).map(([key, spec]) => [key, spec?.type === 'number' || spec?.type === 'integer' ? 0 : spec?.type === 'boolean' ? false : '']));
    return JSON.stringify(value, null, 2);
}

/** Ответ на фразу словами: кто ответит и что колонка скажет или включит. */
function PhraseLines({ report }: { report: PhraseReport }) {
    return (
        <div class="flex flex-col gap-4">
            <span class="t-caption">
                {report.matched ? tr.workshop.matchedLine(report.matched, report.handler ?? '') : tr.workshop.notHeardLine(report.normalized)}
            </span>
            {report.speech && (
                <span class="t-caption">
                    {tr.workshop.deviceSays(device('nom', true), report.speech)}
                </span>
            )}
            {report.play.map((item) => (
                <span key={item.url} class="t-caption">
                    {tr.workshop.willPlay(item.title, item.url, item.format)}
                </span>
            ))}
            {report.letGo && report.matched && <span class="t-caption">{tr.workshop.letGo}</span>}
            {report.error && <span class="t-caption c-dot-offline">{report.error}</span>}
            {report.warnings.map((warning) => (
                <span key={warning} class="t-caption c-pill-yandex-fg">
                    {warning}
                </span>
            ))}
            <LogLines lines={report.log} />
        </div>
    );
}

/** Таймер хода из ответа плагина: `continue.timeout` — `{seconds, handler}` или готовое `{seconds, say}`. */
type TurnTimeout = { seconds: number; handler?: string; say?: string };

function timeoutOf(answer: unknown): TurnTimeout | null {
    const next = answer && typeof answer === 'object' ? (answer as { continue?: unknown }).continue : undefined;
    const timeout = next && typeof next === 'object' ? (next as { timeout?: TurnTimeout }).timeout : undefined;
    return timeout && typeof timeout.seconds === 'number' ? timeout : null;
}

function sayOf(answer: unknown): string | null {
    if (typeof answer === 'string') return answer.trim() || null;
    const say = answer && typeof answer === 'object' ? (answer as { say?: unknown }).say : undefined;
    return typeof say === 'string' && say.trim() ? say.trim() : null;
}

/** Ритм из `contributes.schedules` черновика: что песочница показывает и запускает. */
type DraftRhythm = { id: string; title?: string; at?: string; every?: string; days?: unknown; handler?: string; say?: string };

function rhythmsOf(manifest: string): DraftRhythm[] {
    try {
        const schedules = (JSON.parse(manifest) as { contributes?: { schedules?: unknown } }).contributes?.schedules;
        return Array.isArray(schedules) ? schedules.filter((rhythm): rhythm is DraftRhythm => !!rhythm && typeof rhythm === 'object' && typeof (rhythm as DraftRhythm).id === 'string') : [];
    } catch {
        return [];
    }
}

/** «Время вышло» в песочнице: сколько тишин подряд и что сказал плагин; третья кончает разговор, как в шлюзе. */
type Silences = { timeout: TurnTimeout; missed: number; lines: string[]; done: boolean };

/** Песочница: фразы из манифеста кнопками и все разом, своя фраза, функция с входом по схеме инструмента. */
function Sandbox({ id, manifest, exports }: { id: string; manifest: string; exports: string[] }) {
    const { phrases, tools } = useMemo(() => readManifest(manifest), [manifest]);
    const rhythms = useMemo(() => rhythmsOf(manifest), [manifest]);
    const functions = useMemo(() => [...new Set([...exports, ...tools.map((tool) => tool.handler ?? tool.name)])], [exports, tools]);
    const [busy, setBusy] = useState<string | null>(null);
    const [phraseText, setPhraseText] = useState('');
    const [phrase, setPhrase] = useState<PhraseReport | null>(null);
    const [all, setAll] = useState<Array<{ text: string; report?: PhraseReport; error?: string }> | null>(null);
    const [fn, setFn] = useState('');
    const [input, setInput] = useState('{}');
    const [run, setRun] = useState<RunReport | null>(null);
    const [silences, setSilences] = useState<Silences | null>(null);
    const [error, setError] = useState<string | null>(null);
    useEffect(() => {
        setPhrase(null);
        setAll(null);
        setRun(null);
        setSilences(null);
    }, [id]);
    useEffect(() => {
        if (!fn && functions.length) pick(functions[0]);
    }, [functions.join(',')]);
    const pick = (name: string) => {
        setFn(name);
        setInput(sampleInput(tools.find((tool) => (tool.handler ?? tool.name) === name || tool.name === name)));
    };
    const act = async (what: string, action: () => Promise<void>) => {
        setBusy(what);
        setError(null);
        try {
            await action();
        } catch (e) {
            setError(e instanceof SyntaxError ? tr.workshop.notJson : problemOf(e));
        } finally {
            setBusy(null);
        }
    };
    const tryPhrase = (text: string) =>
        act('phrase', async () => {
            setPhraseText(text);
            const report = await api.tryDraftPhrase(id, text);
            setPhrase(report);
            const timeout = timeoutOf(report.answer);
            setSilences(timeout ? { timeout, missed: 0, lines: [], done: false } : null);
        });
    // Человек молчит: как в шлюзе — функция таймера с {timeout: {seconds, missed}}, готовая фраза кончает разговор,
    // третья тишина подряд кончает его без плагина.
    const silence = () =>
        act('silence', async () => {
            if (!silences) return;
            const { timeout, missed, lines } = silences;
            if (missed >= 2) {
                setSilences({ ...silences, lines: [...lines, tr.workshop.thirdSilence], done: true });
                return;
            }
            if (!timeout.handler) {
                setSilences({ ...silences, lines: [...lines, tr.workshop.timeoutEnds(device(), timeout.say ?? '')], done: true });
                return;
            }
            const report = await api.runDraft(id, timeout.handler, {
                timeout: { seconds: timeout.seconds, missed: missed + 1 },
                request: { text: '', device: 'sandbox', timezone: 'Europe/Moscow', capability: 'plugins' },
            });
            const next = report.ok ? timeoutOf(report.result) : null;
            const said = report.ok ? sayOf(report.result) : null;
            const line = report.ok
                ? tr.workshop.silenceOk(missed + 1, timeout.handler, said ? tr.workshop.silenceWords(said) : tr.workshop.silenceNoWords, next ? tr.workshop.silenceNewTimer(next.seconds) : tr.workshop.silenceEnds)
                : tr.workshop.silenceError(missed + 1, timeout.handler, report.error ?? tr.workshop.noAnswer);
            setSilences({ timeout: next ?? timeout, missed: missed + 1, lines: [...lines, line], done: !next });
        });
    const tryAll = () =>
        act('all', async () => {
            const results: Array<{ text: string; report?: PhraseReport; error?: string }> = [];
            for (const template of phrases) {
                const text = sample(template);
                try {
                    results.push({ text, report: await api.tryDraftPhrase(id, text) });
                } catch (e) {
                    results.push({ text, error: problemOf(e) });
                }
                setAll([...results]);
            }
        });
    return (
        <section class="card glass card-list" aria-labelledby="try-h">
            <div class="card-head">
                <h2 id="try-h" class="t-title-xs m-0">
                    {tr.workshop.sandbox}
                </h2>
                <span class="t-meta">{byMode(tr.workshop.sandboxMetaSpeaker, tr.workshop.sandboxMetaVoice)}</span>
            </div>

            <div class="ws-block">
                <h3 class="t-title-xs m-0">{tr.workshop.phraseHeading}</h3>
                {phrases.length > 0 && (
                    <div class="ws-chips">
                        {phrases.map((template) => (
                            <button key={template} class="pill pill-off cursor-pointer min-h-36" disabled={!!busy} onClick={() => void tryPhrase(sample(template))}>
                                {template}
                            </button>
                        ))}
                        {phrases.length > 1 && <Action tone="on" label={tr.workshop.runAll} busy={busy === 'all'} disabled={!!busy} onClick={() => void tryAll()} />}
                    </div>
                )}
                <div class="ws-row">
                    <input
                        class="field ws-grow"
                        placeholder={tr.workshop.ownPhrase(examplePhrase())}
                        aria-label={tr.workshop.phraseLabel}
                        value={phraseText}
                        onInput={(e) => setPhraseText((e.target as HTMLInputElement).value)}
                        onKeyDown={(e) => e.key === 'Enter' && phraseText.trim() && !busy && void tryPhrase(phraseText)}
                    />
                    <Action label={tr.workshop.runPhrase} busy={busy === 'phrase'} disabled={!!busy || !phraseText.trim()} onClick={() => void tryPhrase(phraseText)} />
                </div>
                {phrase && <PhraseLines report={phrase} />}
                {silences && (
                    <div class="flex flex-col gap-4">
                        <span class="t-caption">
                            {tr.workshop.timerLine(silences.timeout.seconds, silences.timeout.handler ?? tr.workshop.readyPhrase(silences.timeout.say ?? ''))}
                        </span>
                        {silences.lines.map((line) => (
                            <span key={line} class="t-caption">
                                {line}
                            </span>
                        ))}
                        {!silences.done && (
                            <div class="ws-row">
                                <Action label={tr.workshop.stayQuiet(silences.timeout.seconds)} busy={busy === 'silence'} disabled={!!busy} onClick={() => void silence()} />
                            </div>
                        )}
                    </div>
                )}
                {all && (
                    <ol class="ws-results">
                        {all.map((row) => (
                            <li key={row.text} class="ws-result">
                                <span class={`ws-mark ${row.report?.matched && !row.report.error ? 'c-accent-text' : 'c-dot-offline'}`} aria-hidden="true">
                                    {row.report?.matched && !row.report.error ? '✓' : '✗'}
                                </span>
                                <span class="flex flex-col gap-2 min-w-0">
                                    <b class="t-body-sm">«{row.text}»</b>
                                    <span class="t-caption">
                                        {row.error ??
                                            (row.report?.error
                                                ? row.report.error
                                                : row.report?.matched
                                                  ? [row.report.handler && `→ ${row.report.handler}`, row.report.speech && tr.workshop.resultSays(row.report.speech), row.report.play[0] && tr.workshop.resultPlays(row.report.play[0].title)].filter(Boolean).join(' · ') || tr.workshop.resultNoWords
                                                  : tr.workshop.resultNotHeard)}
                                    </span>
                                </span>
                            </li>
                        ))}
                    </ol>
                )}
            </div>

            {rhythms.length > 0 && (
                <div class="ws-block">
                    <h3 class="t-title-xs m-0">{tr.workshop.scheduleHeading}</h3>
                    {rhythms.map((rhythm) => (
                        <div key={rhythm.id} class="ws-row">
                            <span class="t-caption ws-grow">
                                {rhythm.title ?? rhythm.id} · {rhythm.at ? tr.workshop.atTime(rhythm.at) : tr.workshop.everyInterval(rhythm.every ?? '?')}
                                {rhythm.say ? tr.workshop.saysReady(rhythm.say) : rhythm.handler ? ` → ${rhythm.handler}` : ''}
                            </span>
                            {rhythm.handler && (
                                <Action
                                    label={tr.workshop.launch}
                                    busy={busy === `rhythm:${rhythm.id}`}
                                    disabled={!!busy}
                                    onClick={() =>
                                        void act(`rhythm:${rhythm.id}`, async () =>
                                            setRun(await api.runDraft(id, rhythm.handler!, { schedule: { id: rhythm.id, title: rhythm.title ?? rhythm.id, due: Date.now(), late: 0 }, data: null })),
                                        )
                                    }
                                />
                            )}
                        </div>
                    ))}
                </div>
            )}

            <div class="ws-block">
                <h3 class="t-title-xs m-0">{tr.workshop.functionHeading}</h3>
                <div class="ws-row">
                    {functions.length > 0 ? (
                        <select class="field ws-grow" aria-label={tr.workshop.functionLabel} value={fn} onChange={(e) => pick((e.target as HTMLSelectElement).value)}>
                            {functions.map((name) => (
                                <option key={name} value={name}>
                                    {name}
                                </option>
                            ))}
                        </select>
                    ) : (
                        <input class="field ws-grow" placeholder={tr.workshop.functionPlaceholder} aria-label={tr.workshop.functionLabel} value={fn} onInput={(e) => setFn((e.target as HTMLInputElement).value)} />
                    )}
                    <Action label={tr.workshop.launch} busy={busy === 'run'} disabled={!!busy || !fn.trim()} onClick={() => void act('run', async () => setRun(await api.runDraft(id, fn.trim(), JSON.parse(input || '{}'))))} />
                </div>
                <label class="ws-label">
                    <span class="t-caption">{tr.workshop.inputJson}</span>
                    <textarea class="field font-mono ws-code" rows={Math.min(10, Math.max(3, input.split('\n').length))} spellcheck={false} value={input} onInput={(e) => setInput((e.target as HTMLTextAreaElement).value)} />
                </label>
                {run && (
                    <div class="flex flex-col gap-4">
                        <span class={`t-caption ${run.ok ? '' : 'c-dot-offline'}`}>{run.ok ? tr.workshop.answeredIn(run.ms) : tr.workshop.errorLine(run.error ?? '')}</span>
                        {run.ok && <pre class="ws-pre">{JSON.stringify(run.result, null, 2)}</pre>}
                        {run.said.map((said) => (
                            <span key={said} class="t-caption">
                                {tr.workshop.wouldSay(said)}
                            </span>
                        ))}
                        <LogLines lines={run.log} />
                    </div>
                )}
            </div>
            {error && (
                <p class="t-caption m-0 c-dot-offline" role="alert">
                    {error}
                </p>
            )}
        </section>
    );
}

/** Модель агента: «как в «Мозге»» (умный профиль) или своя — провайдер и модель из «Мозга». */
function AgentModelSheet({ current, onSaved, onClose }: { current: WorkshopAgentModel; onSaved: (view: WorkshopAgentModel) => void; onClose: () => void }) {
    const [brain, setBrain] = useState<BrainView | null>(null);
    const [own, setOwn] = useState(!!current.choice);
    const [provider, setProvider] = useState(current.choice?.provider ?? '');
    const [model, setModel] = useState(current.choice?.model ?? '');
    const [busy, setBusy] = useState(false);
    const [error, setError] = useState<string | null>(null);
    useEffect(() => {
        api.brain()
            .then((view) => {
                setBrain(view);
                if (!provider && view.providers[0]) {
                    setProvider(view.providers[0].id);
                    setModel(view.providers[0].models[0] ?? '');
                }
            })
            .catch((e) => setError(problemOf(e)));
    }, []);
    const chosen = brain?.providers.find((entry) => entry.id === provider);
    const save = async () => {
        setBusy(true);
        setError(null);
        try {
            onSaved(await api.setWorkshopAgent(own ? { provider, model: model.trim() } : null));
            onClose();
        } catch (e) {
            setError(problemOf(e));
        } finally {
            setBusy(false);
        }
    };
    const smart = current.smart;
    const buttons = (
            <div class="flex flex-wrap gap-8">
                <Action tone="on" label={tr.workshop.save} busy={busy} disabled={own && (!provider || !model.trim())} onClick={() => void save()} />
                <Action label={tr.workshop.cancel} disabled={busy} onClick={onClose} />
            </div>
    );
    return (
        <Sheet title={tr.workshop.modelSheetTitle} about={tr.workshop.modelSheetAbout} onClose={onClose} footer={buttons}>
            <p class="t-caption m-0 ws-hint">{tr.workshop.modelHint}</p>
            <div role="radiogroup" aria-label={tr.workshop.modelGroupLabel} class="flex flex-col">
                <button role="radio" aria-checked={!own} class="dash-option" onClick={() => setOwn(false)}>
                    <span class="dash-option-text">
                        <b>{tr.workshop.likeBrain}</b>
                        <span class="t-caption">{smart ? tr.workshop.smartProfileWith(smart.model, smart.title) : tr.workshop.smartProfile}</span>
                    </span>
                    <span class={`dash-radio${!own ? ' on' : ''}`} aria-hidden="true">
                        {!own && <IconCheck />}
                    </span>
                </button>
                <button role="radio" aria-checked={own} class="dash-option" disabled={!brain?.providers.length} onClick={() => setOwn(true)}>
                    <span class="dash-option-text">
                        <b>{tr.workshop.ownModel}</b>
                        <span class="t-caption">{brain && !brain.providers.length ? tr.workshop.addProviderFirst : tr.workshop.providerAndModel}</span>
                    </span>
                    <span class={`dash-radio${own ? ' on' : ''}`} aria-hidden="true">
                        {own && <IconCheck />}
                    </span>
                </button>
            </div>
            {own && brain && brain.providers.length > 0 && (
                <div class="ws-form">
                    <label class="ws-label">
                        <span class="t-caption">{tr.workshop.provider}</span>
                        <select
                            class="field"
                            value={provider}
                            onChange={(e) => {
                                const next = (e.target as HTMLSelectElement).value;
                                setProvider(next);
                                setModel(brain.providers.find((entry) => entry.id === next)?.models[0] ?? '');
                            }}
                        >
                            {brain.providers.map((entry) => (
                                <option key={entry.id} value={entry.id}>
                                    {entry.title}
                                </option>
                            ))}
                        </select>
                    </label>
                    <label class="ws-label">
                        <span class="t-caption">{tr.workshop.model}</span>
                        {chosen && chosen.models.length > 0 ? (
                            <select class="field" value={model} onChange={(e) => setModel((e.target as HTMLSelectElement).value)}>
                                {!chosen.models.includes(model) && model && <option value={model}>{model}</option>}
                                {chosen.models.map((name) => (
                                    <option key={name} value={name}>
                                        {name}
                                    </option>
                                ))}
                            </select>
                        ) : (
                            <input class="field" placeholder={tr.workshop.modelIdPlaceholder} value={model} onInput={(e) => setModel((e.target as HTMLInputElement).value)} />
                        )}
                    </label>
                </div>
            )}
            {error && (
                <p class="t-caption m-0 c-dot-offline" role="alert">
                    {error}
                </p>
            )}
        </Sheet>
    );
}

/** Сколько шагов хода работы держит мастерская — как шлюз (activity.rs, KEPT). */
const KEPT_STEPS = 200;

/** Кто попросил агента. */
const ASKED_BY: Record<AgentJob['by'], string> = {
    // Читается при отрисовке: в режиме «Микрофон» колонки нет, а язык пульта может смениться.
    get voice() {
        return byMode(tr.workshop.askedByVoice, tr.workshop.askedByVoiceMic);
    },
    get panel() {
        return tr.workshop.askedByPanel;
    },
};

const clock = (at: number): string => new Date(at).toLocaleTimeString(locale(), { hour: '2-digit', minute: '2-digit', second: '2-digit' });

interface LogRow {
    key: number;
    at: number;
    kind: 'asked' | 'tool' | 'result' | 'said' | 'end';
    /** Кто: «Голосом у колонки», «В пульте», «Агент». */
    who?: string;
    text: string;
    tool?: string;
    /** У вызова — чем он кончился: «✓» или «не вышло: …». */
    mark?: string;
    /** У вызова — что агент получил в ответ (файл, итог проверки; шлюз отдаёт начало): «что увидел». */
    seen?: string;
    failed?: boolean;
}

/**
 * Шаги агента — строками хода работы: вызов вместе с его ответом, сказанное подряд — одной строкой. Кто попросил,
 * известно у работы, что пульт застал (шлюз говорит это о текущей), у прежних — просто «Попросили».
 *
 * Вызовы одного хода агента идут разом, и ответы приходят не по порядку: ответ находит свой вызов по `call` (seq шага
 * вызова). Шлюз постарше `call` не присылает — тогда ответ достаётся вызову без ответа того же инструмента (openCall).
 * Вызова уже нет в списке (он старше двухсот шагов) — неудача встаёт своей строкой, удачный ответ не показывается.
 */
function logRows(steps: AgentStep[], askedBy: Map<number, AgentJob['by']>): LogRow[] {
    const rows: LogRow[] = [];
    /** Строки вызовов по seq их шага. */
    const calls = new Map<number, LogRow>();
    for (const step of steps) {
        const last = rows[rows.length - 1];
        if (step.kind === 'asked') {
            const by = askedBy.get(step.seq);
            rows.push({ key: step.seq, at: step.at, kind: 'asked', who: by ? ASKED_BY[by] : tr.workshop.askedUnknown, text: `«${shortText(step.text, 400)}»` });
        } else if (step.kind === 'tool') {
            const row: LogRow = { key: step.seq, at: step.at, kind: 'tool', tool: step.tool, text: toolWords(step.tool, step.text) };
            rows.push(row);
            calls.set(step.seq, row);
        } else if (step.kind === 'result') {
            const call = typeof step.call === 'number' ? calls.get(step.call) : openCall(rows, step.tool);
            if (call && call.mark === undefined) {
                call.mark = step.failed ? tr.workshop.markFailed(shortText(step.text, 200)) : '✓';
                call.seen = step.text.trim() || undefined;
                call.failed = step.failed;
            } else if (step.failed) {
                rows.push({ key: step.seq, at: step.at, kind: 'result', text: tr.workshop.toolFailed(toolWords(step.tool), shortText(step.text, 200)), failed: true });
            }
        } else if (step.kind === 'said') {
            if (last?.kind === 'said') last.text = `${last.text} ${step.text}`;
            else rows.push({ key: step.seq, at: step.at, kind: 'said', who: tr.workshop.whoAgent, text: step.text });
        } else {
            const text = step.kind === 'failed' ? tr.workshop.endFailed(step.text || tr.workshop.errorDefault) : step.kind === 'stopped' ? step.text || tr.workshop.stoppedDefault : tr.workshop.doneDefault;
            rows.push({ key: step.seq, at: step.at, kind: 'end', text, failed: step.kind === 'failed' });
        }
    }
    return rows;
}

/**
 * Вызов, которому отвечает ответ без `call`: последний вызов того же инструмента, у которого ещё нет ответа, — назад до
 * просьбы, с которой началась эта работа: ответы прежних работ шлюз не пишет (activity.rs, push).
 */
function openCall(rows: LogRow[], tool: string | undefined): LogRow | undefined {
    for (let index = rows.length - 1; index >= 0; index -= 1) {
        const row = rows[index];
        if (row.kind === 'asked') return undefined;
        if (row.kind === 'tool' && row.tool === tool && row.mark === undefined) return row;
    }
    return undefined;
}

/**
 * «Ход работы»: что агент делает с черновиком — кто и о чём попросил (голосом у колонки или в пульте), каждый вызов и
 * чем он кончился, что агент сказал и как кончилась работа, со временем. Новое — внизу, список идёт за ним, пока его не
 * отмотали вверх. Пока агент работает — чем он занят и «Остановить». Черновик, сделанный голосом, показывает так весь
 * разговор у колонки. Работы не было — ничего.
 */
function AgentLog({ id, title }: { id: string; title: string }) {
    const panel = usePanel();
    const version = useSettledVersion();
    const [job, setJob] = useState<AgentJob | null>(null);
    const [steps, setSteps] = useState<AgentStep[]>([]);
    const [stopping, setStopping] = useState(false);
    const [error, setError] = useState<string | null>(null);
    /** Последний прочитанный шаг: дальше читаются только новые. */
    const after = useRef(0);
    const askedBy = useRef(new Map<number, AgentJob['by']>());
    const box = useRef<HTMLDivElement>(null);
    /** Список у нижнего края: новое прокручивает его за собой. */
    const follow = useRef(true);
    const reading = useRef(false);
    const again = useRef(false);

    const read = (): void => {
        // По одному чтению; что пришло за это время — следующим сразу за ним.
        if (reading.current) {
            again.current = true;
            return;
        }
        reading.current = true;
        const from = after.current;
        api.draftActivity(id, from)
            .then((answer) => {
                if (answer.job?.last && answer.job.last.seq < from) {
                    // Шлюз перезапускался и считает шаги заново — всё с начала.
                    after.current = 0;
                    askedBy.current.clear();
                    setSteps([]);
                    again.current = true;
                    return;
                }
                setJob(answer.job);
                const fresh = answer.steps.filter((step) => step.seq > from);
                if (fresh.length === 0) return;
                after.current = fresh[fresh.length - 1].seq;
                // Кто попросил — шлюз говорит о текущей работе: её первый шаг — последняя просьба.
                const asked = [...fresh].reverse().find((step) => step.kind === 'asked');
                if (asked && answer.job) askedBy.current.set(asked.seq, answer.job.by);
                setSteps((before) => [...before, ...fresh].slice(-KEPT_STEPS));
            })
            .catch(() => undefined)
            .finally(() => {
                reading.current = false;
                if (again.current) {
                    again.current = false;
                    read();
                }
            });
    };
    useEffect(() => read(), [version]);
    const busy = !!job?.busy;
    useEffect(() => {
        if (!busy) return;
        const timer = window.setInterval(read, POLL_MS);
        return () => window.clearInterval(timer);
    }, [busy]);
    // За новейшим шагом, а не за длиной: список держит двести шагов, и у долгого черновика длина уже не меняется.
    const newest = steps[steps.length - 1]?.seq;
    useEffect(() => {
        const list = box.current;
        if (list && follow.current) list.scrollTop = list.scrollHeight;
    }, [newest]);
    const now = useSecondTick(busy) + panel.feed.skew;

    if (!job && steps.length === 0) return null;
    const rows = logRows(steps, askedBy.current);
    const stop = async () => {
        if (!(await askToStop(title))) return;
        setStopping(true);
        setError(null);
        try {
            await api.stopDraftAgent(id);
            read();
        } catch (e) {
            setError(problemOf(e));
        } finally {
            setStopping(false);
        }
    };
    return (
        <section class="card glass card-list" aria-labelledby="agent-log-h">
            <div class="card-head">
                <h2 id="agent-log-h" class="t-title-xs m-0">
                    {tr.workshop.logTitle}
                </h2>
                <span class="t-meta">
                    {job?.busy ? tr.workshop.logRunning(elapsed(now - job.since)) : job?.ended ? tr.workshop.logEnded(clock(job.ended)) : ''}
                </span>
            </div>
            <div
                ref={box}
                class="agent-log"
                role="log"
                aria-label={tr.workshop.logStepsLabel}
                tabIndex={0}
                onScroll={() => {
                    const list = box.current;
                    if (list) follow.current = list.scrollHeight - list.scrollTop - list.clientHeight < 24;
                }}
            >
                {rows.map((row) => (
                    <div key={row.key} class={`agent-log-row is-${row.kind}${row.failed ? ' is-failed' : ''}`}>
                        <span class="agent-log-time">{clock(row.at)}</span>
                        <div class="agent-log-text">
                            {row.who && <span class="agent-log-who">{row.who}: </span>}
                            {row.text}
                            {row.mark && <span class="agent-log-mark">{row.mark}</span>}
                            {row.seen && (
                                <details class="agent-log-seen">
                                    <summary>{tr.workshop.seenSummary}</summary>
                                    <pre>{row.seen}</pre>
                                </details>
                            )}
                        </div>
                    </div>
                ))}
            </div>
            {job?.busy && (
                <div class="agent-log-now">
                    <span class="agent-spin" aria-hidden="true" />
                    <span class="flex-1 min-w-0">{doingWords(job.last, now)}</span>
                    <Action label={tr.workshop.stopAction} busy={stopping} onClick={() => void stop()} />
                </div>
            )}
            {error && (
                <p class="t-caption m-0 c-dot-offline" role="alert">
                    {error}
                </p>
            )}
        </section>
    );
}

/**
 * Часть пульта поставленного плагина, когда пульт её подхватит (plugins/loader.ts): новая регистрация на месте
 * `before`. Не дождались — какая есть.
 */
function installedPart(id: string, before: PanelPlugin | undefined, wait = 15_000): Promise<PanelPlugin | undefined> {
    const key = `package.${id}`;
    const find = () => registeredPlugins().find((plugin) => plugin.id === key);
    return new Promise((resolve) => {
        const found = find();
        if (found && found !== before) return resolve(found);
        const done = (plugin: PanelPlugin | undefined) => {
            window.clearTimeout(timer);
            stop();
            resolve(plugin);
        };
        const timer = window.setTimeout(() => done(find()), wait);
        const stop = onRegistryChange(() => {
            const next = find();
            if (next && next !== before) done(next);
        });
    });
}

type ChatEntry = { who: 'me' | 'agent' | 'tool' | 'error'; text: string };

/** Чат с агентом: что сделать — он пишет, проверяет и ставит черновик сам, показывая каждый шаг. */
function AgentChat({ draft, onChanged }: { draft: string; onChanged: () => void }) {
    const [entries, setEntries] = useState<ChatEntry[]>([]);
    // Пришли «Научить»: задание уже в поле — человек читает его и отправляет сам.
    const [text, setText] = useState(() => takeTeaching(draft));
    const [busy, setBusy] = useState(false);
    const [fresh, setFresh] = useState(true);
    const [model, setModel] = useState<WorkshopAgentModel | null>(null);
    const [picking, setPicking] = useState(false);
    const abort = useRef<AbortController | null>(null);
    useEffect(() => {
        setEntries([]);
        setFresh(true);
        return () => abort.current?.abort();
    }, [draft]);
    useEffect(() => {
        api.workshopAgent()
            .then(setModel)
            .catch(() => undefined);
    }, []);
    const add = (entry: ChatEntry) =>
        setEntries((all) => {
            const last = all[all.length - 1];
            if (entry.who === 'agent' && last?.who === 'agent') return [...all.slice(0, -1), { who: 'agent', text: `${last.text} ${entry.text}` }];
            return [...all, entry];
        });
    const send = async () => {
        const message = text.trim();
        if (!message) return;
        setText('');
        add({ who: 'me', text: message });
        setBusy(true);
        abort.current = new AbortController();
        try {
            await workshopChat(
                draft,
                message,
                fresh,
                (event: WorkshopEvent) => {
                    if (event.type === 'text') add({ who: 'agent', text: event.text });
                    if (event.type === 'tool') add({ who: 'tool', text: `→ ${toolWords(event.name, aboutOf(event.input))}` });
                    if (event.type === 'result' && event.isError) add({ who: 'error', text: `${event.name}: ${event.content}` });
                    if (event.type === 'error') add({ who: 'error', text: event.message });
                },
                abort.current.signal,
            );
            setFresh(false);
        } catch (e) {
            if (!(e instanceof DOMException && e.name === 'AbortError')) add({ who: 'error', text: problemOf(e) });
        } finally {
            setBusy(false);
            onChanged();
        }
    };
    // Работа агента идёт и без этого разговора (закрыли окно — продолжается, activity.rs): «Остановить» кончает её на
    // шлюзе, а потом пульт перестаёт слушать.
    const stop = () => {
        add({ who: 'tool', text: tr.workshop.stoppedNote });
        void api
            .stopDraftAgent(draft)
            .catch(() => undefined)
            .finally(() => abort.current?.abort());
    };
    const answers = model?.answers;
    return (
        <>
        <section class="card glass card-list" aria-labelledby="agent-h">
            <div class="card-head">
                <h2 id="agent-h" class="t-title-xs m-0">
                    {tr.workshop.agentTitle}
                </h2>
                {entries.length > 0 && !busy && (
                    <button class="t-caption cursor-pointer underline min-h-44" onClick={() => (setEntries([]), setFresh(true))}>
                        {tr.workshop.newConversation}
                    </button>
                )}
            </div>
            <p class="t-caption m-0 mb-6">{tr.workshop.agentIntro}</p>
            {model && (
                <div class="ws-model">
                    <span class="t-caption">
                        {tr.workshop.modelPrefix}<b>{answers ? `${answers.model} · ${answers.title}` : tr.workshop.modelNotChosen}</b>
                        {model.choice ? tr.workshop.modelOwn : tr.workshop.modelLikeBrain}
                    </span>
                    <button class="t-caption cursor-pointer underline min-h-44" disabled={busy} onClick={() => setPicking(true)}>
                        {tr.workshop.change}
                    </button>
                </div>
            )}
            <div class="ws-chat">
                {entries.map((entry, index) => (
                    <p key={index} class={`ws-msg ws-${entry.who} m-0`}>
                        {entry.text}
                    </p>
                ))}
                {busy && <p class="ws-msg ws-tool m-0">{tr.workshop.working}</p>}
            </div>
            <div class="ws-row mt-8">
                <textarea
                    class="field ws-grow"
                    rows={2}
                    aria-label={tr.workshop.messageLabel}
                    placeholder={tr.workshop.messagePlaceholder}
                    value={text}
                    disabled={busy}
                    onInput={(e) => setText((e.target as HTMLTextAreaElement).value)}
                    onKeyDown={(e) => {
                        if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) void send();
                    }}
                />
                {busy ? <Action label={tr.workshop.stopAction} onClick={stop} /> : <Action tone="on" label={tr.workshop.send} disabled={!text.trim()} onClick={() => void send()} />}
            </div>
        </section>
        {picking && model && <AgentModelSheet current={model} onSaved={setModel} onClose={() => setPicking(false)} />}
        </>
    );
}

/** Черновик открыт: что с ним, файлы, песочница, версии, журнал, агент. */
function Workbench({ id, onClose, onListChanged }: { id: string; onClose: () => void; onListChanged: () => void }) {
    const panel = usePanel();
    // Перемены шлюза — не чаще раза в 0,4 с: каждый шаг агента — перемена, а перечитывать черновик — четыре запроса.
    const version = useSettledVersion();
    const theme = useTheme();
    const [draft, setDraft] = useState<DraftView | null>(null);
    const [installed, setInstalled] = useState<PackageView | null>(null);
    const [path, setPath] = useState('manifest.json');
    const [content, setContent] = useState('');
    const [saved, setSaved] = useState('');
    const [manifest, setManifest] = useState('');
    const [busy, setBusy] = useState<string | null>(null);
    const [error, setError] = useState<string | null>(null);
    const [note, setNote] = useState<{ ok: boolean; text: string } | null>(null);
    const [check, setCheck] = useState<CheckReport | null>(null);
    const [exports, setExports] = useState<string[]>([]);
    const [plan, setPlan] = useState<{ plan: InstallPlan; trial: boolean; station?: string } | null>(null);
    /** Предпросмотр виджета черновика: 0 — не показан, дальше — номер показа (после правки — заново). */
    const [widgetShown, setWidgetShown] = useState(0);
    /** panel.js, который человек разрешил показать: показ выполнит только его (mayRunDraft). */
    const [approved, setApproved] = useState('');
    // Помощник: плагин пишется здесь, а ставится и пробуется на колонке (docs/plans/desk.md, этап 4).
    const helperStation = panel.role.role === 'helper' ? panel.role.station?.key : undefined;
    const [onStation, setOnStation] = useState<string | null | undefined>(undefined);
    useEffect(() => {
        if (!helperStation) return;
        let alive = true;
        void stationVersion(helperStation, id).then((found) => alive && setOnStation(found));
        return () => {
            alive = false;
        };
    }, [helperStation, id, version]);
    const [journal, setJournal] = useState<JournalLine[]>([]);
    const [previewing, setPreviewing] = useState<string | null>(null);

    const readDraft = async () => {
        try {
            const next = await api.openDraft(id);
            setDraft(next);
            setError(null);
            if (next.installed) {
                api.packages()
                    .then((view) => setInstalled(view.packages.find((entry) => entry.id === id) ?? null))
                    .catch(() => undefined);
            } else {
                setInstalled(null);
            }
            return next;
        } catch (e) {
            setError(problemOf(e));
            return null;
        }
    };
    const readManifestFile = () =>
        api
            .readDraftFile(id, 'manifest.json')
            .then((answer) => setManifest(answer.content))
            .catch(() => undefined);
    const readFile = async (file: string) => {
        try {
            const answer = await api.readDraftFile(id, file);
            setPath(file);
            setContent(answer.content);
            setSaved(answer.content);
            if (file === 'manifest.json') setManifest(answer.content);
        } catch (e) {
            setError(problemOf(e));
        }
    };
    const readJournal = () =>
        api
            .draftJournal(id)
            .then((answer) => setJournal(answer.lines.slice(-60)))
            .catch(() => undefined);
    // Функции для песочницы — из тихой проверки: показывается только та, что по кнопке.
    const readExports = () =>
        api
            .checkDraft(id)
            .then((report) => setExports(report.exports))
            .catch(() => undefined);
    useEffect(() => {
        setCheck(null);
        setNote(null);
        setPlan(null);
        setWidgetShown(0);
        void readDraft().then((next) => {
            void readFile('manifest.json');
            if (next?.kind !== 'theme') void readExports();
        });
    }, [id]);
    useEffect(() => void readJournal(), [id, version]);
    // Агент, голосовой режим или конец пробы поменяли черновик (workshop.changed, packages.changed): список файлов,
    // версии и открытый файл, если в нём нет несохранённого, — заново.
    useEffect(() => {
        if (!draft) return;
        void readDraft();
        void readManifestFile();
        if (content === saved) void readFile(path);
    }, [version]);
    // Проба кончилась по времени: шлюз вернул прежнюю — перечитать, даже если события не было.
    useEffect(() => {
        if (!draft?.trial) return;
        const wait = Date.parse(draft.trial.until) - Date.now() + 3000;
        const timer = window.setTimeout(() => void readDraft(), Math.max(1000, wait));
        return () => window.clearTimeout(timer);
    }, [draft?.trial?.until]);
    // Примерка уходит вместе с мастерской.
    useEffect(() => () => {
        if (previewing) {
            unregisterPlugin(PREVIEW);
            setTheme(previewing);
        }
    }, [previewing]);
    // Несохранённое в файле: закрыть окно, уйти в другой раздел или к черновикам — сначала вопрос (shell/leave.ts).
    useLeaveGuard(content !== saved, () =>
        confirmAsk({ title: tr.workshop.leaveTitle, text: tr.workshop.leaveText(path), ok: tr.workshop.leaveOk, danger: true }),
    );

    const act = async (what: string, action: () => Promise<void>) => {
        setBusy(what);
        setError(null);
        try {
            await action();
        } catch (e) {
            setError(problemOf(e));
        } finally {
            setBusy(null);
        }
    };
    const saveNow = async () => {
        if (content === saved) return;
        setDraft(await api.writeDraftFile(id, path, content));
        setSaved(content);
        if (path === 'manifest.json') setManifest(content);
        onListChanged();
        if (draft?.kind !== 'theme') void readExports();
    };
    const save = () => act('save', saveNow);
    /** «Поставить…» и «Проба…»: сначала сохранить, потом показать, что изменится. */
    const ask = (trial: boolean) =>
        act(trial ? 'ask-trial' : 'ask', async () => {
            await saveNow();
            setPlan({ plan: await api.installPlan(id), trial });
        });
    const finished = async (report: InstallReport, trial = false) => {
        setPlan(null);
        // Проба рассказывает о себе плашкой — строка под кнопками её бы повторила.
        setNote(trial && report.installed ? null : { ok: report.installed, text: report.message });
        await readDraft();
        void readJournal();
        onListChanged();
    };
    /**
     * Виджет стола поставленного черновика — сразу и на стол, когда пульт подхватит часть плагина: так на стол встаёт и
     * виджет, сделанный голосом у колонки.
     */
    const putOnDesk = async (before: PanelPlugin | undefined, said: string) => {
        void syncPackages();
        const role = panel.role.role;
        const part = await installedPart(id, before);
        const widgets = (part?.slots?.['desk.widgets'] ?? []).filter((widget) => !widget.roles || widget.roles.includes(role));
        if (widgets.length === 0) return;
        const desk = deskActions(role);
        for (const widget of widgets) desk.add(`package.${id}.${widget.id}`);
        const names = widgets.map((widget) => `«${widget.title ?? draft?.title ?? id}»`).join(', ');
        setNote({ ok: true, text: widgets.length > 1 ? tr.workshop.onDeskMany(said, names) : tr.workshop.onDeskOne(said, names) });
    };
    const doInstall = () =>
        act('install', async () => {
            if (plan?.station) {
                await finished(await stationInstall(plan.station, id));
                // Виджеты стола помощника зовут плагин колонки, если он там стоит (sdk, useApi): список — заново.
                forgetStationPackages();
                setOnStation(await stationVersion(plan.station, id));
            } else {
                const before = registeredPlugins().find((plugin) => plugin.id === `package.${id}`);
                const report = await api.installDraft(id, true);
                await finished(report);
                // В окне колонки у помощника её стол не здесь — туда не ставим.
                if (report.installed && hasPanel && !VIA) void putOnDesk(before, report.message);
            }
        });
    const doTrial = (minutes: number) =>
        act('trial', async () => {
            if (plan?.station) {
                await finished(await stationTrial(plan.station, id, minutes), false);
                forgetStationPackages();
            } else await finished(await api.startTrial(id, true, minutes), true);
        });
    /** «Поставить на колонку…», «Проба на колонке…»: черновик уходит в мастерскую колонки, там — что изменится. */
    const askStation = (trial: boolean) =>
        helperStation &&
        act(trial ? 'ask-trial' : 'ask', async () => {
            await saveNow();
            await pushDraft(helperStation, id);
            setPlan({ plan: await stationPlan(helperStation, id), trial, station: helperStation });
        });
    const endTrial = (keep: boolean) =>
        act(keep ? 'keep' : 'end', async () => {
            const answer = await api.endTrial(id, keep);
            setNote({ ok: true, text: answer.message });
            await readDraft();
            void readJournal();
            onListChanged();
        });
    const rollback = async () => {
        const ready = installed?.versions.filter((version) => version.problems.length === 0) ?? [];
        if (!installed || ready.length < 2) return;
        const yes = await confirmAsk({
            title: tr.workshop.rollbackTitle(ready[1].version),
            text: tr.workshop.rollbackText(ready[0].version),
            ok: tr.workshop.rollbackOk,
        });
        if (!yes) return;
        void act('rollback', async () => {
            const view = await api.rollbackPackage(id);
            setInstalled(view);
            setNote({ ok: true, text: tr.workshop.nowWorking(view.version ?? '') });
            await readDraft();
            void readJournal();
        });
    };
    const preview = () =>
        act('preview', async () => {
            const found = normalizeTheme(JSON.parse((await api.readDraftFile(id, 'manifest.json')).content)?.contributes?.themes?.[0]);
            if (!found) throw new Error(tr.workshop.noThemeInManifest);
            const previous = previewing ?? theme.theme.id;
            registerPlugin({ id: PREVIEW, themes: [{ ...found, id: `preview-${found.id}`, title: tr.workshop.themeTryTitle(found.title) }] });
            setTheme(`preview-${found.id}`);
            setPreviewing(previous);
        });
    const endPreview = () => {
        if (!previewing) return;
        unregisterPlugin(PREVIEW);
        setTheme(previewing);
        setPreviewing(null);
    };
    const pickFile = async (file: string, create = false) => {
        if (content !== saved && !(await confirmAsk({ title: tr.workshop.leaveTitle, text: tr.workshop.leaveText(path), ok: tr.workshop.leaveOk, danger: true })))
            return;
        if (create) {
            setPath(file);
            setContent('');
            setSaved('');
        } else {
            void readFile(file);
        }
    };

    const files = draft?.files ?? [];
    const plugin = draft?.kind !== 'theme';
    // Виджет стола в черновике (panel.js): показать, как он встанет на стол.
    const hasPanel = files.some((file) => file.path === 'panel.js');
    const dirty = content !== saved;
    // Редактор — по высоте кода (до 70% экрана, дальше прокрутка внутри).
    const rows = Math.min(48, Math.max(12, content.split('\n').length + 1));
    return (
        <>
            <section class="card glass card-list" aria-labelledby="draft-h">
                <div class="card-head">
                    <h2 id="draft-h" class="t-title-xs m-0">
                        {draft?.title ?? id}
                    </h2>
                    <button class="t-caption cursor-pointer underline min-h-44" onClick={() => void mayLeave().then((ok) => ok && onClose())}>
                        {tr.workshop.toDrafts}
                    </button>
                </div>
                <p class="t-caption m-0">
                    {[
                        plugin ? tr.workshop.kindPlugin : tr.workshop.kindTheme,
                        draft?.version && tr.workshop.draftVersion(draft.version),
                        helperStation
                            ? onStation === undefined
                                ? null
                                : onStation
                                  ? tr.workshop.statusOnSpeaker(onStation)
                                  : tr.workshop.statusNotOnSpeaker
                            : draft?.installed
                              ? tr.workshop.statusOnGateway(draft.installed)
                              : tr.workshop.notInstalledShort,
                        helperStation && draft?.installed ? tr.workshop.statusOnComputer(draft.installed) : null,
                    ]
                        .filter(Boolean)
                        .join(' · ')}
                </p>
                {helperStation && plugin && <p class="t-caption m-0">{tr.workshop.writtenHere}</p>}
                {draft?.problems.map((problem) => (
                    <span key={problem} class="t-caption c-dot-offline">
                        {problem}
                    </span>
                ))}
                {draft?.trial && <TrialBanner draft={draft} busy={busy} onEnd={(keep) => void endTrial(keep)} />}
                <div class="flex flex-wrap gap-8 mt-8">
                    <Action label={tr.workshop.check} busy={busy === 'check'} disabled={!!busy} onClick={() => void act('check', async () => (await saveNow(), setCheck(await api.checkDraft(id))))} />
                    {helperStation && plugin ? (
                        <>
                            <Action tone="on" label={tr.workshop.installOnSpeaker} busy={busy === 'ask'} disabled={!!busy} onClick={() => void askStation(false)} />
                            <Action label={tr.workshop.trialOnSpeaker} busy={busy === 'ask-trial'} disabled={!!busy} onClick={() => void askStation(true)} />
                            <Action label={tr.workshop.installOnComputer} disabled={!!busy} onClick={() => void ask(false)} />
                        </>
                    ) : (
                        <>
                            <Action tone="on" label={tr.workshop.installEllipsis} busy={busy === 'ask'} disabled={!!busy} onClick={() => void ask(false)} />
                            {plugin && !draft?.trial && <Action label={byMode(tr.workshop.trialEllipsisSpeaker, tr.workshop.trialEllipsisVoice)} busy={busy === 'ask-trial'} disabled={!!busy} onClick={() => void ask(true)} />}
                        </>
                    )}
                    {!plugin &&
                        (previewing ? <Action label={tr.workshop.untryTheme} onClick={endPreview} /> : <Action label={tr.workshop.tryTheme} busy={busy === 'preview'} disabled={!!busy} onClick={() => void preview()} />)}
                    {plugin && (
                        <Action
                            label={widgetShown ? tr.workshop.showAgain : tr.workshop.showWidget}
                            disabled={!!busy}
                            onClick={() =>
                                void act('widget', async () => {
                                    await saveNow();
                                    // panel.js работает в своём окошке, без прав пульта; карточку рисует сам пульт.
                                    if (hasPanel) {
                                        const code = await mayRunDraft(id);
                                        if (code === null) return;
                                        setApproved(code);
                                    }
                                    setWidgetShown((n) => n + 1);
                                })
                            }
                        />
                    )}
                    <Action
                        label={tr.workshop.deleteDraft}
                        disabled={!!busy}
                        onClick={async () =>
                            (await confirmAsk({ title: tr.workshop.deleteTitle, text: tr.workshop.deleteText, ok: tr.workshop.deleteOk, danger: true })) &&
                            void act('delete', async () => {
                                await api.deleteDraft(id);
                                onListChanged();
                                onClose();
                            })
                        }
                    />
                </div>
                {widgetShown > 0 &&
                    (hasPanel ? (
                        <DraftWidgetPreview id={id} nonce={widgetShown} code={approved} />
                    ) : (
                        <DraftCardPreview id={id} nonce={widgetShown} trying={{ example: false }} fallback={<p class="t-caption m-0">{tr.workshop.noCardNoWindow}</p>} />
                    ))}
                {check && <CheckLines report={check} />}
                {note && (
                    <span class={`t-caption ${note.ok ? 'c-accent-text' : 'c-dot-offline'}`} role="status">
                        {note.text}
                    </span>
                )}
                {error && (
                    <p class="t-caption m-0 c-dot-offline" role="alert">
                        {error}
                    </p>
                )}
            </section>

            <AgentLog key={id} id={id} title={draft?.title ?? id} />

            <section class="card glass card-list" aria-labelledby="files-h">
                <div class="card-head">
                    <h2 id="files-h" class="t-title-xs m-0">
                        {tr.workshop.filesTitle}
                    </h2>
                    <span class="t-meta">{dirty ? tr.workshop.unsaved : tr.workshop.savedState}</span>
                </div>
                <div class="ws-tabs" role="tablist" aria-label={tr.workshop.filesLabel}>
                    {files.map((file) => (
                        <button key={file.path} role="tab" aria-selected={file.path === path} class={`ws-tab${file.path === path ? ' on' : ''}`} onClick={() => file.path !== path && pickFile(file.path)}>
                            {file.path}
                            {file.path === path && dirty && <span class="ws-dirty" aria-label={tr.workshop.unsavedLabel} />}
                        </button>
                    ))}
                    {!files.some((file) => file.path === path) && (
                        <button role="tab" aria-selected={true} class="ws-tab on">
                            {path}
                            {dirty && <span class="ws-dirty" aria-label={tr.workshop.unsavedLabel} />}
                        </button>
                    )}
                    {plugin &&
                        ['backend.js', 'panel.js'].filter((file) => file !== path && !files.some((entry) => entry.path === file)).map((file) => (
                            <button key={file} class="ws-tab ws-tab-new" onClick={() => pickFile(file, true)}>
                                + {file}
                            </button>
                        ))}
                </div>
                <CodeEditor
                    value={content}
                    path={path}
                    rows={rows}
                    onInput={setContent}
                    onKeyDown={(e) => {
                        if (e.key === 's' && (e.metaKey || e.ctrlKey)) {
                            e.preventDefault();
                            if (!busy && dirty) void save();
                        }
                    }}
                />
                <div class="flex flex-wrap items-center gap-8">
                    <Action tone="on" label={tr.workshop.savePath(path)} busy={busy === 'save'} disabled={!!busy || !dirty} onClick={() => void save()} />
                    {dirty && <Action label={tr.workshop.revert} disabled={!!busy} onClick={() => setContent(saved)} />}
                </div>
            </section>

            {plugin && <Sandbox id={id} manifest={manifest} exports={exports} />}

            {installed && draft?.trial && <VersionsCard entry={installed} busy={busy} trial onRollback={rollback} />}

            <HistoryCard
                id={id}
                mode="draft"
                onChanged={() => {
                    void readDraft();
                    void readJournal();
                    onListChanged();
                }}
            />

            {draft?.installed && plugin && (
                <section class="card glass card-list" aria-labelledby="journal-h">
                    <div class="card-head">
                        <h2 id="journal-h" class="t-title-xs m-0">
                            {tr.workshop.journalTitle}
                        </h2>
                        <button class="t-caption cursor-pointer underline min-h-44" onClick={() => void readJournal()}>
                            {tr.workshop.refresh}
                        </button>
                    </div>
                    {journal.length === 0 ? <p class="t-caption m-0">{tr.workshop.journalQuiet}</p> : <LogLines lines={journal} />}
                    {journal.length > 0 && <span class="t-meta">{tr.workshop.journalLast(timeAgo(journal[journal.length - 1].at))}</span>}
                </section>
            )}

            <AgentChat draft={id} onChanged={() => void readDraft().then(() => (content === saved ? readFile(path) : readManifestFile()))} />

            {plan && <InstallSheet plan={plan.plan} trial={plan.trial} busy={busy} onInstall={() => void doInstall()} onTrial={(minutes) => void doTrial(minutes)} onClose={() => setPlan(null)} />}
        </>
    );
}

/** Вкладка «Мастерская» раздела «Плагины». */
export function WorkshopScreen() {
    const version = useSettledVersion();
    const linked = useLinkedItem('skills', 'workshop');
    const [drafts, setDrafts] = useState<DraftView[] | null>(null);
    const [error, setError] = useState<string | null>(null);
    const read = () =>
        api
            .workshop()
            .then((view) => {
                setDrafts(view.drafts);
                setError(null);
            })
            .catch((e) => setError(problemOf(e)));
    useEffect(() => {
        if (isAdmin()) void read();
    }, [version]);
    if (!isAdmin()) {
        return (
            <section class="card glass">
                <p class="t-caption m-0">{tr.workshop.adminOnly}</p>
            </section>
        );
    }
    return (
        <>
            {error && (
                <section class="card glass" role="alert">
                    <p class="t-caption m-0 c-dot-offline">{error}</p>
                </section>
            )}
            {linked.item ? (
                <Workbench id={linked.item} onClose={linked.close} onListChanged={() => void read()} />
            ) : (
                <>
                    <DraftList drafts={drafts ?? []} onOpen={(id) => linked.open(id)} onMade={(draft) => (void read(), linked.open(draft.id))} />
                    <section class="card glass card-list" aria-labelledby="history-room-h">
                        <div class="card-head">
                            <h2 id="history-room-h" class="t-title-xs m-0">
                                {tr.workshop.historyTitle}
                            </h2>
                        </div>
                        <p class="t-caption m-0">{tr.workshop.historyAbout}</p>
                        <HistoryRoomLine titled />
                    </section>
                </>
            )}
        </>
    );
}
