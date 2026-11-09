// «Мастерская» («Плагины» → «Мастерская»; docs/plans/plugins.md, этап 6): плагин или тема пишется в черновике —
// агентом в чате или руками —, проверяется в песочнице шлюза и ставится новой версией рядом с прежней. «Поставить»
// всегда сначала показывает, что изменится (версия, права, хосты, что даёт), и ставится только по «да» администратора:
// агент и голос этого не могут. Плагин можно попробовать на колонке, не ставя насовсем: проба идёт на шлюзе несколько
// минут, потом прежняя версия возвращается сама. Тему — примерить в пульте. Версии на шлюзе и откат — здесь же.

import { useEffect, useMemo, useRef, useState } from 'preact/hooks';

import {
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
import { useLinkedItem, usePanel } from '../shell/context';
import { registerPlugin, unregisterPlugin } from '../shell/registry';
import { setTheme, useTheme } from '../shell/theme';
import { Theme, normalizeTheme } from '../shell/themes';
import { timeAgo } from '../time';
import { Action } from '../ui/cards';
import { Sheet } from '../ui/sheet';
import { permissionTitle } from './plugins';
import { ask as confirmAsk } from '../ui/confirm';

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
        look: base.look ?? 'glass',
        layout: base.layout,
        density: base.density,
        font: base.font,
        radius: base.radius,
        palette: base.palette,
    };
}

/** Черновики и новый черновик. */
function DraftList({ drafts, onOpen, onMade }: { drafts: DraftView[]; onOpen: (id: string) => void; onMade: (draft: DraftView) => void }) {
    const linked = useLinkedItem('plugins', 'workshop');
    const theme = useTheme();
    // «Вид пульта» → «Сделать свою тему»: форма темы сразу, на основе выбранной.
    const base = linked.query.new === 'theme' ? (theme.themes.find((entry) => entry.id === linked.query.base) ?? theme.theme) : null;
    const [making, setMaking] = useState<'plugin' | 'theme' | 'openapi' | null>(base ? 'theme' : null);
    const [id, setId] = useState(base ? idOf(`my-${base.id}`) : '');
    const [title, setTitle] = useState(base ? `Моя тема (${base.title})` : '');
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
                    <h3 class="t-title-xs m-0">{making === 'theme' ? (base ? `Своя тема на основе «${base.title}»` : 'Новая тема') : making === 'openapi' ? 'Плагин из описания API' : 'Новый плагин'}</h3>
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
function RightsLines({ plan }: { plan: InstallPlan }) {
    const rights = plan.kept.filter((name) => !name.startsWith('net:'));
    const hosts = plan.kept.filter((name) => name.startsWith('net:')).map((name) => name.slice(4));
    const nothing = !plan.added.length && !plan.removed.length && !plan.hostsAdded.length && !plan.hostsRemoved.length && !plan.kept.length;
    return (
        <div class="flex flex-col gap-6">
            <h3 class="t-title-xs m-0">Права</h3>
            {nothing && <span class="t-caption">Особых прав не просит.</span>}
            {(plan.added.length > 0 || plan.hostsAdded.length > 0) && (
                <div class="ws-chips">
                    <span class="t-caption">{plan.first ? 'Просит:' : 'Новые:'}</span>
                    {plan.added.map((name) => (
                        <span key={name} class="pill pill-yandex">
                            {permissionTitle(name)}
                        </span>
                    ))}
                    {plan.hostsAdded.map((host) => (
                        <span key={host} class="pill pill-yandex">
                            сеть: {host}
                        </span>
                    ))}
                </div>
            )}
            {(plan.removed.length > 0 || plan.hostsRemoved.length > 0) && (
                <div class="ws-chips">
                    <span class="t-caption">Больше не нужны:</span>
                    {plan.removed.map((name) => (
                        <span key={name} class="pill pill-off line-through">
                            {permissionTitle(name)}
                        </span>
                    ))}
                    {plan.hostsRemoved.map((host) => (
                        <span key={host} class="pill pill-off line-through">
                            сеть: {host}
                        </span>
                    ))}
                </div>
            )}
            {(rights.length > 0 || hosts.length > 0) && (
                <span class="t-caption">
                    Без изменений: {[...rights.map(permissionTitle), ...hosts.map((host) => `сеть: ${host}`)].join(', ')}.
                </span>
            )}
        </div>
    );
}

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

/** Песочница: фразы из манифеста кнопками и все разом, своя фраза, функция с входом по схеме инструмента. */
function Sandbox({ id, manifest, exports }: { id: string; manifest: string; exports: string[] }) {
    const { phrases, tools } = useMemo(() => readManifest(manifest), [manifest]);
    const functions = useMemo(() => [...new Set([...exports, ...tools.map((tool) => tool.handler ?? tool.name)])], [exports, tools]);
    const [busy, setBusy] = useState<string | null>(null);
    const [phraseText, setPhraseText] = useState('');
    const [phrase, setPhrase] = useState<PhraseReport | null>(null);
    const [all, setAll] = useState<Array<{ text: string; report?: PhraseReport; error?: string }> | null>(null);
    const [fn, setFn] = useState('');
    const [input, setInput] = useState('{}');
    const [run, setRun] = useState<RunReport | null>(null);
    const [error, setError] = useState<string | null>(null);
    useEffect(() => {
        setPhrase(null);
        setAll(null);
        setRun(null);
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
            setPhrase(await api.tryDraftPhrase(id, text));
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
        <Sheet title="Модель агента" about="Только для агента мастерской — «Мозг» не меняется" onClose={onClose} footer={buttons}>
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

type ChatEntry = { who: 'me' | 'agent' | 'tool' | 'error'; text: string };

/** Чат с агентом: что сделать — он пишет, проверяет и ставит черновик сам, показывая каждый шаг. */
function AgentChat({ draft, onChanged }: { draft: string; onChanged: () => void }) {
    const [entries, setEntries] = useState<ChatEntry[]>([]);
    const [text, setText] = useState('');
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
                    if (event.type === 'tool') add({ who: 'tool', text: `→ ${event.name}${event.input.path ? ` ${String(event.input.path)}` : ''}` });
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
                {busy ? <Action label="Стоп" onClick={() => abort.current?.abort()} /> : <Action tone="on" label="Отправить" disabled={!text.trim()} onClick={() => void send()} />}
            </div>
        </section>
        {picking && model && <AgentModelSheet current={model} onSaved={setModel} onClose={() => setPicking(false)} />}
        </>
    );
}

/** Черновик открыт: что с ним, файлы, песочница, версии, журнал, агент. */
function Workbench({ id, onClose, onListChanged }: { id: string; onClose: () => void; onListChanged: () => void }) {
    const panel = usePanel();
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
    const [plan, setPlan] = useState<{ plan: InstallPlan; trial: boolean } | null>(null);
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
        void readDraft().then((next) => {
            void readFile('manifest.json');
            if (next?.kind !== 'theme') void readExports();
        });
    }, [id]);
    useEffect(() => void readJournal(), [id, panel.version]);
    // Агент, голосовой режим или конец пробы поменяли черновик (workshop.changed, packages.changed): список файлов,
    // версии и открытый файл, если в нём нет несохранённого, — заново.
    useEffect(() => {
        if (!draft) return;
        void readDraft();
        void readManifestFile();
        if (content === saved) void readFile(path);
    }, [panel.version]);
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
    const doInstall = () => act('install', async () => finished(await api.installDraft(id, true)));
    const doTrial = (minutes: number) => act('trial', async () => finished(await api.startTrial(id, true, minutes), true));
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
                    <button class="t-caption cursor-pointer underline min-h-44" onClick={onClose}>
                        к черновикам
                    </button>
                </div>
                <p class="t-caption m-0">
                    {[plugin ? 'плагин' : 'тема', draft?.version && `черновик ${draft.version}`, draft?.installed ? `на шлюзе ${draft.installed}` : 'не поставлен'].filter(Boolean).join(' · ')}
                </p>
                {draft?.problems.map((problem) => (
                    <span key={problem} class="t-caption c-dot-offline">
                        {problem}
                    </span>
                ))}
                {draft?.trial && <TrialBanner draft={draft} busy={busy} onEnd={(keep) => void endTrial(keep)} />}
                <div class="flex flex-wrap gap-8 mt-8">
                    <Action label="Проверить" busy={busy === 'check'} disabled={!!busy} onClick={() => void act('check', async () => (await saveNow(), setCheck(await api.checkDraft(id))))} />
                    <Action tone="on" label="Поставить…" busy={busy === 'ask'} disabled={!!busy} onClick={() => void ask(false)} />
                    {plugin && !draft?.trial && <Action label="Проба на колонке…" busy={busy === 'ask-trial'} disabled={!!busy} onClick={() => void ask(true)} />}
                    {!plugin &&
                        (previewing ? <Action label="Снять примерку" onClick={endPreview} /> : <Action label="Примерить тему" busy={busy === 'preview'} disabled={!!busy} onClick={() => void preview()} />)}
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
                <textarea
                    class="field font-mono ws-editor"
                    rows={rows}
                    spellcheck={false}
                    autocapitalize="off"
                    autocomplete="off"
                    aria-label={`Файл ${path}`}
                    value={content}
                    onInput={(e) => setContent((e.target as HTMLTextAreaElement).value)}
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

            {installed && <VersionsCard entry={installed} busy={busy} trial={!!draft?.trial} onRollback={rollback} />}

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
    const panel = usePanel();
    const linked = useLinkedItem('plugins', 'workshop');
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
    }, [panel.version]);
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
                <DraftList drafts={drafts ?? []} onOpen={(id) => linked.open(id)} onMade={(draft) => (void read(), linked.open(draft.id))} />
            )}
        </>
    );
}
