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
import { VIA } from '../via';
import { RightsLines } from '../ui/rights-lines';
import { ask as confirmAsk } from '../ui/confirm';
import { pushDraft, stationInstall, stationPlan, stationTrial, stationVersion } from './to-station';
import { DraftWidgetPreview, mayRunDraft } from './widget-agent';
import { HistoryCard, HistoryRoomLine } from './history';
import { CodeEditor } from '../ui/code-editor';

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
        about: `Своя тема на основе «${base.title}»`,
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
    const [title, setTitle] = useState(base ? `Моя тема (${base.title})` : teach ? `«${teach}»` : '');
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
                    `Научи колонку отвечать на «${teach}»: какой фразой её спрашивают, что ей нужно узнать и что сказать в ответ. Если нужны данные из интернета — скажи, откуда.`,
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
                    Черновики
                </h2>
                <span class="t-meta">{drafts.length}</span>
            </div>
            <p class="t-caption m-0 mb-6">Плагин или тема пишется здесь — агентом в чате или руками, проверяется в песочнице и ставится новой версией рядом с прежней.</p>
            {drafts.length === 0 && <p class="t-caption m-0 mb-6">Пока пусто.</p>}
            {drafts.map((draft) => (
                <div key={draft.id} class="pkg-row">
                    <div class="pkg-main">
                        <button class="pkg-text text-left cursor-pointer" onClick={() => onOpen(draft.id)}>
                            <span class="pkg-title">
                                {draft.title ?? draft.id}
                                <span class="pill pill-off">{draft.kind === 'theme' ? 'тема' : 'плагин'}</span>
                                {draft.trial && <span class="pill pill-agent">проба</span>}
                                {draft.problems.length > 0 && <span class="pill pill-error">манифест с ошибкой</span>}
                            </span>
                            <span class="t-caption">
                                {[draft.id, draft.version && `черновик ${draft.version}`, draft.installed ? `стоит ${draft.installed}` : 'не поставлен'].filter(Boolean).join(' · ')}
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
                                ? `Своя тема на основе «${base.title}»`
                                : 'Новая тема'
                            : making === 'openapi'
                              ? 'Плагин из описания API'
                              : making === 'quiz'
                                ? 'Викторина на время: 10 секунд на ответ, «Время вышло!»'
                                : 'Новый плагин'}
                    </h3>
                    <label class="ws-label">
                        <span class="t-caption">id — латиница, цифры, «-»</span>
                        <input class="field" value={id} onInput={(e) => setId((e.target as HTMLInputElement).value)} />
                    </label>
                    <label class="ws-label">
                        <span class="t-caption">Название</span>
                        <input class="field" value={title} onInput={(e) => setTitle((e.target as HTMLInputElement).value)} />
                    </label>
                    {making === 'openapi' && (
                        <label class="ws-label">
                            <span class="t-caption">Описание API (OpenAPI) в JSON</span>
                            <textarea class="field font-mono ws-code" rows={8} spellcheck={false} value={openapi} onInput={(e) => setOpenapi((e.target as HTMLTextAreaElement).value)} />
                        </label>
                    )}
                    <div class="flex flex-wrap gap-8">
                        <Action tone="on" label="Создать" busy={busy} disabled={!id.trim()} onClick={() => void make()} />
                        <Action label="Отмена" disabled={busy} onClick={() => (setMaking(null), base && linked.close())} />
                    </div>
                </div>
            ) : (
                <div class="flex flex-wrap gap-8 mt-8">
                    <Action tone="on" label="Новый плагин" onClick={() => setMaking('plugin')} />
                    <Action label="Викторина на время" onClick={() => setMaking('quiz')} />
                    <Action label="Новая тема" onClick={() => setMaking('theme')} />
                    <Action label="Из описания API" onClick={() => setMaking('openapi')} />
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
                {report.ok ? `Проверка прошла за ${report.ms} мс${report.exports.length ? `: функции ${report.exports.join(', ')}` : ''}.` : 'Проверка не прошла.'}
            </span>
            {report.manifest.map((problem) => (
                <span key={problem} class="t-caption c-dot-offline">
                    Манифест: {problem}
                </span>
            ))}
            {report.code && <span class="t-caption c-dot-offline">Код: {report.code}</span>}
            {report.missing.length > 0 && <span class="t-caption c-dot-offline">Нет функций: {report.missing.join(', ')}</span>}
            {report.warnings.map((warning) => (
                <span key={warning} class="t-caption c-pill-yandex-fg">
                    {warning}
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
    const title = trial ? `Проба версии ${plan.to}` : plan.first ? `Поставить «${plan.title}» ${plan.to}?` : `Поставить версию ${plan.to}?`;
    const about = plan.first ? (plugin ? 'Первая установка: код будет работать в шлюзе с этими правами' : 'Первая установка: тема появится в «Виде пульта»') : `Сейчас стоит ${plan.from} → станет ${plan.to}`;
    const buttons = (
            <div class="flex flex-wrap gap-8">
                {trial ? (
                    <>
                        <Action tone="on" label={asks ? `Одобрить и пробовать ${minutes} мин` : `Пробовать ${minutes} мин`} busy={busy === 'trial'} disabled={!!busy} onClick={() => onTrial(minutes)} />
                        <Action label={`Поставить ${plan.to} насовсем`} busy={busy === 'install'} disabled={!!busy} onClick={onInstall} />
                    </>
                ) : (
                    <>
                        <Action tone="on" label={asks ? `Одобрить и поставить ${plan.to}` : `Поставить ${plan.to}`} busy={busy === 'install'} disabled={!!busy} onClick={onInstall} />
                        {plugin && <Action label={`Проба ${minutes} мин`} busy={busy === 'trial'} disabled={!!busy} onClick={() => onTrial(minutes)} />}
                    </>
                )}
                <Action label="Отмена" disabled={!!busy} onClick={onClose} />
            </div>
    );
    return (
        <Sheet title={title} about={about} onClose={onClose} footer={buttons}>
            {trial && (
                <p class="t-body-sm m-0">
                    {plan.from
                        ? `Черновик поработает на шлюзе вместо ${plan.from}, потом ${plan.from} вернётся сама. Говорите колонке как обычно.`
                        : 'Плагин поработает на шлюзе, потом уйдёт сам. Говорите колонке как обычно.'}
                </p>
            )}
            <RightsLines plan={plan} />
            {(plan.phrases.length > 0 || plan.tools.length > 0) && (
                <div class="flex flex-col gap-4">
                    <h3 class="t-title-xs m-0">Что даёт</h3>
                    {plan.phrases.length > 0 && (
                        <span class="t-caption">
                            Фразы: {plan.phrases.slice(0, 6).map((phrase) => `«${phrase}»`).join(', ')}
                            {plan.phrases.length > 6 ? ` и ещё ${plan.phrases.length - 6}` : ''}
                        </span>
                    )}
                    {plan.tools.length > 0 && <span class="t-caption">Инструменты «Мозга»: {plan.tools.join(', ')}</span>}
                </div>
            )}
            {plugin && (
                <p class="t-caption m-0">
                    Бюджет: память {plan.budget.memoryMb} МБ, хранилище {plan.budget.storageMb} МБ. Не поднимется — работает прежняя версия.
                </p>
            )}
            {!trial && plan.from && (
                <p class="t-caption m-0">
                    Прежняя {plan.from} останется на шлюзе — вернуть её можно здесь же, в «Версиях» (хранятся последние {plan.keptVersions}).
                    {plugin && !plan.enabled ? ' Плагин выключен — новая версия тоже будет выключена.' : ''}
                </p>
            )}
            {plugin && (
                <div class="flex flex-col gap-6">
                    {!trial && <h3 class="t-title-xs m-0">Или сначала попробовать</h3>}
                    <div class="ws-chips" role="radiogroup" aria-label="Сколько длится проба">
                        <span class="t-caption">Проба на колонке:</span>
                        {TRIAL_MINUTES.map((value) => (
                            <button key={value} role="radio" aria-checked={minutes === value} class={`pill ${minutes === value ? 'pill-own' : 'pill-off'} cursor-pointer min-h-36`} onClick={() => setMinutes(value)}>
                                {value} мин
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
    return minutes <= 1 ? 'меньше минуты' : `${minutes} мин`;
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
                <b>Проба на колонке:</b> работает {trial.version}
                {trial.previous ? ` вместо ${trial.previous}` : ''} — ещё {left(trial.until, now)}. Потом {trial.previous ? `сама вернётся ${trial.previous}` : 'плагин сам уйдёт'}.
            </span>
            <div class="flex flex-wrap gap-8">
                <Action tone="on" label="Оставить эту версию" busy={busy === 'keep'} disabled={!!busy} onClick={() => onEnd(true)} />
                <Action label={trial.previous ? `Вернуть ${trial.previous} сейчас` : 'Убрать сейчас'} busy={busy === 'end'} disabled={!!busy} onClick={() => onEnd(false)} />
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
                    Версии на шлюзе
                </h2>
                <span class="t-meta">{entry.versions.length}</span>
            </div>
            <ol class="ws-versions">
                {entry.versions.map((version, index) => (
                    <li key={version.version} class="ws-version">
                        <span class="font-mono">{version.version}</span>
                        {version.version === entry.version && <span class={`pill ${entry.enabled ? 'pill-own' : 'pill-off'}`}>{entry.enabled ? 'работает' : 'выбрана, плагин выключен'}</span>}
                        {index === 0 && version.version !== entry.version && <span class="pill pill-off">новейшая</span>}
                        {version.problems.length > 0 && <span class="t-caption c-dot-offline">{version.problems.join('; ')}</span>}
                    </li>
                ))}
            </ol>
            {canRollback && (
                <div class="mt-8">
                    <Action label={`Откатить к ${ready[1].version}`} busy={busy === 'rollback'} disabled={!!busy} onClick={onRollback} />
                </div>
            )}
            {trial && <p class="t-caption m-0 mt-8">Пока идёт проба, откат — кнопкой на плашке пробы.</p>}
            {!trial && ready.length <= 1 && <p class="t-caption m-0 mt-8">Откатываться пока не на что: на шлюзе одна рабочая версия.</p>}
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
                {report.matched ? `Подошла фраза «${report.matched}» → ${report.handler}` : `Плагин этих слов не слышит («${report.normalized}»): ответит встроенное.`}
            </span>
            {report.speech && <span class="t-caption">Колонка скажет: «{report.speech}»</span>}
            {report.play.map((item) => (
                <span key={item.url} class="t-caption">
                    Включит: {item.title} — {item.url} ({item.format})
                </span>
            ))}
            {report.letGo && report.matched && <span class="t-caption">Плагин отпустил фразу: ответит встроенный навык той же возможности.</span>}
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
            setError(e instanceof SyntaxError ? 'Вход — не JSON.' : problemOf(e));
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
                setSilences({ ...silences, lines: [...lines, 'Третья тишина подряд: шлюз скажет «Похоже, никого нет — заканчиваю» и закончит разговор.'], done: true });
                return;
            }
            if (!timeout.handler) {
                setSilences({ ...silences, lines: [...lines, `Время вышло: колонка скажет «${timeout.say ?? ''}», и разговор закончится.`], done: true });
                return;
            }
            const report = await api.runDraft(id, timeout.handler, {
                timeout: { seconds: timeout.seconds, missed: missed + 1 },
                request: { text: '', device: 'sandbox', timezone: 'Europe/Moscow', capability: 'plugins' },
            });
            const next = report.ok ? timeoutOf(report.result) : null;
            const said = report.ok ? sayOf(report.result) : null;
            const line = report.ok
                ? `${missed + 1}-я тишина → ${timeout.handler}: ${said ? `«${said}»` : 'без слов'}${next ? ` · новый таймер ${next.seconds} с` : ' · разговор кончится'}`
                : `${missed + 1}-я тишина → ${timeout.handler}: ошибка — ${report.error ?? 'нет ответа'}`;
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
                    Песочница
                </h2>
                <span class="t-meta">без колонки, ничего не ставит</span>
            </div>

            <div class="ws-block">
                <h3 class="t-title-xs m-0">Фраза</h3>
                {phrases.length > 0 && (
                    <div class="ws-chips">
                        {phrases.map((template) => (
                            <button key={template} class="pill pill-off cursor-pointer min-h-36" disabled={!!busy} onClick={() => void tryPhrase(sample(template))}>
                                {template}
                            </button>
                        ))}
                        {phrases.length > 1 && <Action tone="on" label="Прогнать все" busy={busy === 'all'} disabled={!!busy} onClick={() => void tryAll()} />}
                    </div>
                )}
                <div class="ws-row">
                    <input
                        class="field ws-grow"
                        placeholder="Своя фраза: «Алиса, включи радио джаз»"
                        aria-label="Фраза"
                        value={phraseText}
                        onInput={(e) => setPhraseText((e.target as HTMLInputElement).value)}
                        onKeyDown={(e) => e.key === 'Enter' && phraseText.trim() && !busy && void tryPhrase(phraseText)}
                    />
                    <Action label="Прогнать" busy={busy === 'phrase'} disabled={!!busy || !phraseText.trim()} onClick={() => void tryPhrase(phraseText)} />
                </div>
                {phrase && <PhraseLines report={phrase} />}
                {silences && (
                    <div class="flex flex-col gap-4">
                        <span class="t-caption">
                            Таймер хода: {silences.timeout.seconds} с → {silences.timeout.handler ?? `готовая фраза «${silences.timeout.say ?? ''}»`}
                        </span>
                        {silences.lines.map((line) => (
                            <span key={line} class="t-caption">
                                {line}
                            </span>
                        ))}
                        {!silences.done && (
                            <div class="ws-row">
                                <Action label={`Промолчать ${silences.timeout.seconds} с`} busy={busy === 'silence'} disabled={!!busy} onClick={() => void silence()} />
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
                                                  ? [row.report.handler && `→ ${row.report.handler}`, row.report.speech && `скажет «${row.report.speech}»`, row.report.play[0] && `включит ${row.report.play[0].title}`].filter(Boolean).join(' · ') || 'ответил без слов'
                                                  : 'не услышал — ответит встроенное')}
                                    </span>
                                </span>
                            </li>
                        ))}
                    </ol>
                )}
            </div>

            {rhythms.length > 0 && (
                <div class="ws-block">
                    <h3 class="t-title-xs m-0">Расписание</h3>
                    {rhythms.map((rhythm) => (
                        <div key={rhythm.id} class="ws-row">
                            <span class="t-caption ws-grow">
                                {rhythm.title ?? rhythm.id} · {rhythm.at ? `в ${rhythm.at}` : `каждые ${rhythm.every ?? '?'}`}
                                {rhythm.say ? ` · скажет готовое «${rhythm.say}» — плагин не нужен` : rhythm.handler ? ` → ${rhythm.handler}` : ''}
                            </span>
                            {rhythm.handler && (
                                <Action
                                    label="Запустить"
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
                <h3 class="t-title-xs m-0">Функция</h3>
                <div class="ws-row">
                    {functions.length > 0 ? (
                        <select class="field ws-grow" aria-label="Функция" value={fn} onChange={(e) => pick((e.target as HTMLSelectElement).value)}>
                            {functions.map((name) => (
                                <option key={name} value={name}>
                                    {name}
                                </option>
                            ))}
                        </select>
                    ) : (
                        <input class="field ws-grow" placeholder="Имя функции из backend.js" aria-label="Функция" value={fn} onInput={(e) => setFn((e.target as HTMLInputElement).value)} />
                    )}
                    <Action label="Запустить" busy={busy === 'run'} disabled={!!busy || !fn.trim()} onClick={() => void act('run', async () => setRun(await api.runDraft(id, fn.trim(), JSON.parse(input || '{}'))))} />
                </div>
                <label class="ws-label">
                    <span class="t-caption">Вход (JSON)</span>
                    <textarea class="field font-mono ws-code" rows={Math.min(10, Math.max(3, input.split('\n').length))} spellcheck={false} value={input} onInput={(e) => setInput((e.target as HTMLTextAreaElement).value)} />
                </label>
                {run && (
                    <div class="flex flex-col gap-4">
                        <span class={`t-caption ${run.ok ? '' : 'c-dot-offline'}`}>{run.ok ? `Ответила за ${run.ms} мс:` : `Ошибка: ${run.error}`}</span>
                        {run.ok && <pre class="ws-pre">{JSON.stringify(run.result, null, 2)}</pre>}
                        {run.said.map((said) => (
                            <span key={said} class="t-caption">
                                Сказала бы: «{said}»
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
                <Action tone="on" label="Сохранить" busy={busy} disabled={own && (!provider || !model.trim())} onClick={() => void save()} />
                <Action label="Отмена" disabled={busy} onClick={onClose} />
            </div>
    );
    return (
        <Sheet title="Модель агента" about="Только для агента мастерской — мозг персонажа не меняется" onClose={onClose} footer={buttons}>
            <p class="t-caption m-0 ws-hint">Код лучше пишут сильные модели: быстрые и маленькие чаще ошибаются в коде и в манифесте. Проверка и песочница поймают ошибку, но чинить её придётся дольше.</p>
            <div role="radiogroup" aria-label="Модель агента" class="flex flex-col">
                <button role="radio" aria-checked={!own} class="dash-option" onClick={() => setOwn(false)}>
                    <span class="dash-option-text">
                        <b>Как в «Мозге»</b>
                        <span class="t-caption">{smart ? `Умный профиль: ${smart.model} · ${smart.title}` : 'Умный профиль «Мозга»'}</span>
                    </span>
                    <span class={`dash-radio${!own ? ' on' : ''}`} aria-hidden="true">
                        {!own && <IconCheck />}
                    </span>
                </button>
                <button role="radio" aria-checked={own} class="dash-option" disabled={!brain?.providers.length} onClick={() => setOwn(true)}>
                    <span class="dash-option-text">
                        <b>Своя модель</b>
                        <span class="t-caption">{brain && !brain.providers.length ? 'Сначала добавьте провайдера в «Мозге».' : 'Провайдер из «Мозга» и его модель'}</span>
                    </span>
                    <span class={`dash-radio${own ? ' on' : ''}`} aria-hidden="true">
                        {own && <IconCheck />}
                    </span>
                </button>
            </div>
            {own && brain && brain.providers.length > 0 && (
                <div class="ws-form">
                    <label class="ws-label">
                        <span class="t-caption">Провайдер</span>
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
                        <span class="t-caption">Модель</span>
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
                            <input class="field" placeholder="id модели" value={model} onInput={(e) => setModel((e.target as HTMLInputElement).value)} />
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
const ASKED_BY: Record<AgentJob['by'], string> = { voice: 'Голосом у колонки', panel: 'В пульте' };

const clock = (at: number): string => new Date(at).toLocaleTimeString('ru-RU', { hour: '2-digit', minute: '2-digit', second: '2-digit' });

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
            rows.push({ key: step.seq, at: step.at, kind: 'asked', who: by ? ASKED_BY[by] : 'Попросили', text: `«${shortText(step.text, 400)}»` });
        } else if (step.kind === 'tool') {
            const row: LogRow = { key: step.seq, at: step.at, kind: 'tool', tool: step.tool, text: toolWords(step.tool, step.text) };
            rows.push(row);
            calls.set(step.seq, row);
        } else if (step.kind === 'result') {
            const call = typeof step.call === 'number' ? calls.get(step.call) : openCall(rows, step.tool);
            if (call && call.mark === undefined) {
                call.mark = step.failed ? `не вышло: ${shortText(step.text, 200)}` : '✓';
                call.seen = step.text.trim() || undefined;
                call.failed = step.failed;
            } else if (step.failed) {
                rows.push({ key: step.seq, at: step.at, kind: 'result', text: `${toolWords(step.tool)} — не вышло: ${shortText(step.text, 200)}`, failed: true });
            }
        } else if (step.kind === 'said') {
            if (last?.kind === 'said') last.text = `${last.text} ${step.text}`;
            else rows.push({ key: step.seq, at: step.at, kind: 'said', who: 'Агент', text: step.text });
        } else {
            const text = step.kind === 'failed' ? `Не вышло: ${step.text || 'ошибка'}` : step.kind === 'stopped' ? step.text || 'Остановлено' : 'Готово';
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
                    Ход работы
                </h2>
                <span class="t-meta">
                    {job?.busy ? `идёт ${elapsed(now - job.since)}` : job?.ended ? `закончено в ${clock(job.ended)}` : ''}
                </span>
            </div>
            <div
                ref={box}
                class="agent-log"
                role="log"
                aria-label="Шаги агента"
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
                                    <summary>что увидел</summary>
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
                    <Action label="Остановить" busy={stopping} onClick={() => void stop()} />
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
        add({ who: 'tool', text: 'остановлено' });
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
                    Агент
                </h2>
                {entries.length > 0 && !busy && (
                    <button class="t-caption cursor-pointer underline min-h-44" onClick={() => (setEntries([]), setFresh(true))}>
                        новый разговор
                    </button>
                )}
            </div>
            <p class="t-caption m-0 mb-6">Скажите, что плагин должен уметь, — агент напишет, проверит в песочнице и предложит поставить. Ставить и давать права — вам.</p>
            {model && (
                <div class="ws-model">
                    <span class="t-caption">
                        Модель: <b>{answers ? `${answers.model} · ${answers.title}` : 'не выбрана'}</b>
                        {model.choice ? ' — своя для мастерской' : ' — как в «Мозге»'}
                    </span>
                    <button class="t-caption cursor-pointer underline min-h-44" disabled={busy} onClick={() => setPicking(true)}>
                        сменить
                    </button>
                </div>
            )}
            <div class="ws-chat">
                {entries.map((entry, index) => (
                    <p key={index} class={`ws-msg ws-${entry.who} m-0`}>
                        {entry.text}
                    </p>
                ))}
                {busy && <p class="ws-msg ws-tool m-0">работаю…</p>}
            </div>
            <div class="ws-row mt-8">
                <textarea
                    class="field ws-grow"
                    rows={2}
                    aria-label="Сообщение агенту"
                    placeholder="Например: сделай навык «какая погода на даче» по Open-Meteo"
                    value={text}
                    disabled={busy}
                    onInput={(e) => setText((e.target as HTMLTextAreaElement).value)}
                    onKeyDown={(e) => {
                        if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) void send();
                    }}
                />
                {busy ? <Action label="Остановить" onClick={stop} /> : <Action tone="on" label="Отправить" disabled={!text.trim()} onClick={() => void send()} />}
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
        confirmAsk({ title: 'Уйти без сохранения?', text: `В ${path} есть несохранённое.`, ok: 'Уйти', danger: true }),
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
        setNote({ ok: true, text: `${said} ${widgets.length > 1 ? `Виджеты ${names} на столе.` : `Виджет ${names} на столе.`}` });
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
            title: `Вернуть версию ${ready[1].version}?`,
            text: `Версия ${ready[0].version} уйдёт со шлюза; черновик останется как есть.`,
            ok: 'Вернуть',
        });
        if (!yes) return;
        void act('rollback', async () => {
            const view = await api.rollbackPackage(id);
            setInstalled(view);
            setNote({ ok: true, text: `Работает ${view.version}.` });
            await readDraft();
            void readJournal();
        });
    };
    const preview = () =>
        act('preview', async () => {
            const found = normalizeTheme(JSON.parse((await api.readDraftFile(id, 'manifest.json')).content)?.contributes?.themes?.[0]);
            if (!found) throw new Error('В манифесте нет темы, которую можно примерить.');
            const previous = previewing ?? theme.theme.id;
            registerPlugin({ id: PREVIEW, themes: [{ ...found, id: `preview-${found.id}`, title: `${found.title} (примерка)` }] });
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
        if (content !== saved && !(await confirmAsk({ title: `Уйти без сохранения?`, text: `В ${path} есть несохранённое.`, ok: 'Уйти', danger: true })))
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
                        к черновикам
                    </button>
                </div>
                <p class="t-caption m-0">
                    {[
                        plugin ? 'плагин' : 'тема',
                        draft?.version && `черновик ${draft.version}`,
                        helperStation
                            ? onStation === undefined
                                ? null
                                : onStation
                                  ? `на колонке сейчас ${onStation} — останется рядом для отката`
                                  : 'на колонке не стоит'
                            : draft?.installed
                              ? `на шлюзе ${draft.installed}`
                              : 'не поставлен',
                        helperStation && draft?.installed ? `на этом компьютере ${draft.installed}` : null,
                    ]
                        .filter(Boolean)
                        .join(' · ')}
                </p>
                {helperStation && plugin && <p class="t-caption m-0">Пишется здесь — работает на колонке, и без компьютера.</p>}
                {draft?.problems.map((problem) => (
                    <span key={problem} class="t-caption c-dot-offline">
                        {problem}
                    </span>
                ))}
                {draft?.trial && <TrialBanner draft={draft} busy={busy} onEnd={(keep) => void endTrial(keep)} />}
                <div class="flex flex-wrap gap-8 mt-8">
                    <Action label="Проверить" busy={busy === 'check'} disabled={!!busy} onClick={() => void act('check', async () => (await saveNow(), setCheck(await api.checkDraft(id))))} />
                    {helperStation && plugin ? (
                        <>
                            <Action tone="on" label="Поставить на колонку…" busy={busy === 'ask'} disabled={!!busy} onClick={() => void askStation(false)} />
                            <Action label="Проба на колонке…" busy={busy === 'ask-trial'} disabled={!!busy} onClick={() => void askStation(true)} />
                            <Action label="Поставить на этот компьютер…" disabled={!!busy} onClick={() => void ask(false)} />
                        </>
                    ) : (
                        <>
                            <Action tone="on" label="Поставить…" busy={busy === 'ask'} disabled={!!busy} onClick={() => void ask(false)} />
                            {plugin && !draft?.trial && <Action label="Проба на колонке…" busy={busy === 'ask-trial'} disabled={!!busy} onClick={() => void ask(true)} />}
                        </>
                    )}
                    {!plugin &&
                        (previewing ? <Action label="Снять примерку" onClick={endPreview} /> : <Action label="Примерить тему" busy={busy === 'preview'} disabled={!!busy} onClick={() => void preview()} />)}
                    {plugin && hasPanel && (
                        <Action
                            label={widgetShown ? 'Показать заново' : 'Показать виджет'}
                            disabled={!!busy}
                            onClick={() =>
                                void act('widget', async () => {
                                    await saveNow();
                                    // panel.js выполнится в пульте с правами вошедшего: сначала спросить — у черновика,
                                    // сделанного голосом, каждый раз, у прочих — когда его код не тот, что разрешали.
                                    const code = await mayRunDraft(id);
                                    if (code === null) return;
                                    setApproved(code);
                                    setWidgetShown((n) => n + 1);
                                })
                            }
                        />
                    )}
                    <Action
                        label="Удалить черновик"
                        disabled={!!busy}
                        onClick={async () =>
                            (await confirmAsk({ title: 'Удалить черновик?', text: 'Поставленный пакет останется.', ok: 'Удалить', danger: true })) &&
                            void act('delete', async () => {
                                await api.deleteDraft(id);
                                onListChanged();
                                onClose();
                            })
                        }
                    />
                </div>
                {widgetShown > 0 && <DraftWidgetPreview id={id} nonce={widgetShown} code={approved} />}
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
                        Файлы
                    </h2>
                    <span class="t-meta">{dirty ? 'не сохранено · ⌘S' : 'сохранено'}</span>
                </div>
                <div class="ws-tabs" role="tablist" aria-label="Файлы черновика">
                    {files.map((file) => (
                        <button key={file.path} role="tab" aria-selected={file.path === path} class={`ws-tab${file.path === path ? ' on' : ''}`} onClick={() => file.path !== path && pickFile(file.path)}>
                            {file.path}
                            {file.path === path && dirty && <span class="ws-dirty" aria-label="не сохранено" />}
                        </button>
                    ))}
                    {!files.some((file) => file.path === path) && (
                        <button role="tab" aria-selected={true} class="ws-tab on">
                            {path}
                            {dirty && <span class="ws-dirty" aria-label="не сохранено" />}
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
                    <Action tone="on" label={`Сохранить ${path}`} busy={busy === 'save'} disabled={!!busy || !dirty} onClick={() => void save()} />
                    {dirty && <Action label="Вернуть как было" disabled={!!busy} onClick={() => setContent(saved)} />}
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
                            Журнал плагина
                        </h2>
                        <button class="t-caption cursor-pointer underline min-h-44" onClick={() => void readJournal()}>
                            обновить
                        </button>
                    </div>
                    {journal.length === 0 ? <p class="t-caption m-0">Пока тихо: вызовов не было.</p> : <LogLines lines={journal} />}
                    {journal.length > 0 && <span class="t-meta">последняя запись {timeAgo(journal[journal.length - 1].at)}</span>}
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
                <p class="t-caption m-0">Мастерская — для администратора: черновик становится кодом в шлюзе.</p>
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
                                История приложений
                            </h2>
                        </div>
                        <p class="t-caption m-0">Каждый шаг и каждая версия хранятся: черновик и поставленное можно вернуть к любой записи.</p>
                        <HistoryRoomLine titled />
                    </section>
                </>
            )}
        </>
    );
}
