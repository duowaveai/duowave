// Окно установленного пакета (docs/plans/plugins.md, «Экран «Плагины» и три вида пакетов», этап 5): что с ним сейчас
// (работает, ошибка, выключен), его память против бюджета, вызовы и последняя ошибка, права и хосты, что он даёт
// шлюзу, его настройки — и кнопки по виду: системный не выключить и не удалить, встроенный — только выключить или
// сделать свою копию, свой — всё, включая «Открыть в мастерской». Шлюз проверяет каждое действие сам.

import { useEffect, useState } from 'preact/hooks';

import { LaterCommandView, PackageOrigin, PackageView, MidiView, PluginFilesView, ScheduleJob, ScheduleSpec, ScheduleView, api, problemOf, sessionStore, can } from '../api';
import { formatNumber, locale, tr } from '../i18n';
import { usePanel } from '../shell/context';
import { registeredPlugins } from '../shell/registry';
import { STARTING_SHORTCUTS, useDeskLayout } from '../shell/widgets';
import { timeAgo } from '../time';
import { Action } from '../ui/cards';
import { SaveBar, Sheet, SkeletonRows, WinRow, WinSection } from '../ui/sheet';
import { AppIcon } from '../ui/app-icon';
import { SettingsFields } from '../ui/settings-fields';
import { Switch } from '../ui/switch';
import { useMicHold } from '../voice/phrases';
import { placeOf } from './catalog';
import { pluginMemoryLine } from './memory';
import { HistoryCard } from './history';
import { ask } from '../ui/confirm';
import { saveLabel } from '../via';
import { LinksBlock } from '../bus/links-block';
import { rightTitle } from '../bus/rights';
import { byMode } from '../words';

/** Вид пакета по тому, откуда он, — на метке и в окне. */
export const ORIGIN_TITLES: Record<PackageOrigin, string> = {
    get system() {
        return tr.plugins.originSystem;
    },
    get bundled() {
        return tr.plugins.originBundled;
    },
    get user() {
        return tr.plugins.originUser;
    },
    get catalog() {
        return tr.plugins.originCatalog;
    },
};

const ORIGIN_RULES: Record<PackageOrigin, string> = {
    // Читается при отрисовке: в режиме «Микрофон» колонки нет.
    get system() {
        return byMode(tr.plugins.ruleSystemSpeaker, tr.plugins.ruleSystemOwn);
    },
    get bundled() {
        return tr.plugins.ruleBundled;
    },
    get user() {
        return tr.plugins.ruleUser;
    },
    get catalog() {
        return tr.plugins.ruleCatalog;
    },
};


/** Мегабайты одной цифрой после запятой: «2,1». */
const mb = (bytes: number): string => formatNumber(bytes / (1024 * 1024), { minimumFractionDigits: 1, maximumFractionDigits: 1 });

/** Картинки плагина в его окне (этап 10): сколько их и сколько места. На компьютере предел у каждого плагина свой
 *  (budget.storageMb); на колонке предел общий на все плагины — показываем, сколько из него занято всеми вместе. */
function FilesLine({ id }: { id: string }) {
    const panel = usePanel();
    const [view, setView] = useState<PluginFilesView | null>(null);
    const [allBytes, setAllBytes] = useState<number | null>(null);
    useEffect(() => {
        api.packageFiles(id)
            .then((answer) => {
                setView(answer);
                if (answer.onStation)
                    api.pluginFilesSummary()
                        .then((summary) => setAllBytes(summary.bytes))
                        .catch(() => setAllBytes(null));
            })
            .catch(() => setView(null));
    }, [id, panel.version]);
    if (!view || view.files.length === 0) return null;
    return (
        <WinRow
            title={tr.plugins.filesLine(view.files.length, mb(view.bytes), view.onStation ? null : mb(view.limitBytes))}
            sub={view.onStation ? tr.plugins.filesStation(mb(view.limitBytes), allBytes != null ? mb(allBytes) : null) : undefined}
        />
    );
}

/** MIDI-клавиатура приложения (docs/plans/midi.md): слушает ли её приложение сейчас и где. Клавиатура открыта, только
 *  пока открыто окно или виджет приложения; вне дома её не слышно. */
function MidiLine({ id }: { id: string }) {
    const panel = usePanel();
    const [view, setView] = useState<MidiView | null>(null);
    useEffect(() => {
        api.midi()
            .then(setView)
            .catch(() => setView(null));
    }, [id, panel.version]);
    if (!view) return null;
    if (!view.available) return <WinRow title="MIDI" sub={tr.plugins.midiUnavailable(view.why ?? tr.plugins.midiNone)} />;
    const listening = view.listeners.filter((listener) => listener.package === id);
    const where = (zone: string): string => (zone === 'local' ? tr.plugins.zoneLocal : zone === 'home' ? tr.plugins.zoneHome : tr.plugins.zoneAway);
    return (
        <WinRow
            title="MIDI"
            sub={listening.length > 0
                ? tr.plugins.midiListening(
                      [...new Set(listening.flatMap((listener) => listener.devices))].join(', '),
                      [...new Set(listening.map((listener) => where(listener.zone)))].join(', '),
                  )
                : view.devices.length > 0
                  ? tr.plugins.midiIdle(view.devices.map((device) => device.name).join(', '))
                  : tr.plugins.midiNotConnected}
        />
    );
}

/** Право словами — одна функция на весь пульт (bus/rights.ts). */
export const permissionTitle = rightTitle;

const isAdmin = (): boolean => can('plugins.manage') && sessionStore.get()?.role === 'admin';

/**
 * Настройки приложения из манифеста (строка, выбор, число, да/нет, секрет): поля — во вкладке «Настройки», а
 * «Сохранить» — в нижней полосе окна (SaveBar), только когда что-то изменили.
 */
function useSettingsForm(entry: PackageView, onSaved: (view: PackageView) => void) {
    const fields = entry.settings ?? [];
    const initial = (): Record<string, unknown> => Object.fromEntries(fields.map((field) => [field.key, field.value ?? field.default ?? null]));
    const [values, setValues] = useState<Record<string, unknown>>(initial);
    const [busy, setBusy] = useState(false);
    const [note, setNote] = useState<{ ok: boolean; text: string } | null>(null);
    // Пока открыто поле личного, микрофон пульта выключен: ключ вслух не называют.
    const [privateOpen, setPrivateOpen] = useState(false);
    useMicHold(privateOpen);
    useEffect(() => setValues(initial()), [entry.id, entry.version]);
    const dirty = fields.length > 0 && JSON.stringify(values) !== JSON.stringify(initial());
    const save = async () => {
        setBusy(true);
        setNote(null);
        try {
            onSaved(await api.setPackageSettings(entry.id, values));
            setNote({ ok: true, text: tr.plugins.saved });
        } catch (e) {
            setNote({ ok: false, text: problemOf(e) });
        } finally {
            setBusy(false);
        }
    };
    const form =
        fields.length === 0 ? (
            <p class="win-text m-0">{tr.plugins.noSettings}</p>
        ) : (
            <div class="flex flex-col gap-12">
                <SettingsFields fields={fields} values={values} busy={busy} onChange={(next) => (setNote(null), setValues(next))} onPrivate={setPrivateOpen} />
                {note && (
                    <span class={`t-caption ${note.ok ? 'c-accent-text' : 'c-dot-offline'}`} role={note.ok ? 'status' : 'alert'}>
                        {note.text}
                    </span>
                )}
            </div>
        );
    return { has: fields.length > 0, form, dirty, busy, save, revert: () => setValues(initial()) };
}

/** Время колонки из её ISO-строки: «13.11 в 07:30». */
function stationTime(iso: string): string {
    return tr.plugins.stationTime(`${iso.slice(8, 10)}.${iso.slice(5, 7)}`, iso.slice(11, 16));
}

/** Срок словами: «по будням в 07:30», «каждые 2 ч с 09:00 до 21:00», «один раз». */
export function scheduleWords(spec: ScheduleSpec): string {
    if (spec.type === 'at') {
        const days =
            spec.days === 'daily'
                ? tr.plugins.daysDaily
                : spec.days === 'weekdays'
                  ? tr.plugins.daysWeekdays
                  : spec.days === 'weekends'
                    ? tr.plugins.daysWeekends
                    : spec.days.on.map((day) => tr.plugins.weekdays[day]).join(', ');
        return tr.plugins.atTime(days, spec.at.startsWith('{') ? tr.plugins.timeFromSettings : spec.at);
    }
    if (spec.type === 'every') {
        const every = spec.minutes % 60 === 0 ? tr.plugins.everyHours(spec.minutes / 60) : tr.plugins.everyMinutes(spec.minutes);
        return tr.plugins.everyRange(every, spec.from, spec.to);
    }
    return tr.plugins.once;
}

/** Расписание плагина (docs/plans/plugin-schedules.md, этап 3): ритмы манифеста и задачи из кода — когда, следующий
 *  срок, чем кончился прошлый раз; администратор запускает сейчас, ставит на паузу и снимает задачи из кода. */
function ScheduleBlock({ id, admin }: { id: string; admin: boolean }) {
    const panel = usePanel();
    // undefined — читается (место держит скелет), null — не прочиталось или задач нет.
    const [view, setView] = useState<ScheduleView | null | undefined>(undefined);
    const [busy, setBusy] = useState<string | null>(null);
    const [note, setNote] = useState<{ ok: boolean; text: string } | null>(null);
    useEffect(() => {
        let alive = true;
        api.packageSchedule(id)
            .then((answer) => alive && setView(answer))
            .catch(() => alive && setView((was) => (was === undefined ? null : was)));
        return () => {
            alive = false;
        };
    }, [id, panel.version]);
    if (view === undefined)
        return (
            <WinSection title={tr.plugins.schedule}>
                <SkeletonRows rows={1} />
            </WinSection>
        );
    if (!view || view.jobs.length === 0) return null;
    const act = async (what: string, action: () => Promise<ScheduleView | { outcome: string }>) => {
        setBusy(what);
        setNote(null);
        try {
            const answer = await action();
            if ('jobs' in answer) setView(answer);
            else {
                setNote({ ok: true, text: answer.outcome || tr.plugins.done });
                setView(await api.packageSchedule(id));
            }
        } catch (e) {
            setNote({ ok: false, text: problemOf(e) });
        } finally {
            setBusy(null);
        }
    };
    const line = (job: ScheduleJob): string =>
        [
            scheduleWords(job.spec),
            job.source === 'code' ? tr.plugins.fromCode : null,
            job.say ? tr.plugins.willSay(job.say) : null,
            job.paused ? tr.plugins.paused : job.next ? tr.plugins.nextAt(stationTime(job.next)) : null,
            job.last ? tr.plugins.lastTime(job.last.outcome) : null,
        ]
            .filter(Boolean)
            .join(' · ');
    return (
        <WinSection title={tr.plugins.schedule} aside={!view.on ? tr.plugins.schedulesOff : undefined}>
            <div class="win-group">
                {view.jobs.map((job) => (
                    <div key={job.id} class="win-row is-col">
                        <span class="win-row-t">{job.title}</span>
                        <span class="win-row-s">{line(job)}</span>
                        {job.problem && (
                            <span class="win-row-s c-dot-offline" role="alert">
                                {job.problem}
                            </span>
                        )}
                        {admin && (
                            <div class="flex flex-wrap items-center gap-8">
                                <button class="btn btn-plain btn-sm" disabled={!!busy} aria-busy={busy === `run:${job.id}` || undefined} onClick={() => void act(`run:${job.id}`, () => api.runScheduleJob(id, job.id))}>
                                    {tr.plugins.runNow}
                                </button>
                                <button class="btn btn-plain btn-sm" disabled={!!busy} aria-busy={busy === `pause:${job.id}` || undefined} onClick={() => void act(`pause:${job.id}`, () => api.pauseScheduleJob(id, job.id, !job.paused))}>
                                    {job.paused ? tr.plugins.resume : tr.plugins.pause}
                                </button>
                                {job.source === 'code' && (
                                    <button class="btn btn-quiet btn-sm" disabled={!!busy} aria-busy={busy === `remove:${job.id}` || undefined} onClick={() => void act(`remove:${job.id}`, () => api.removeScheduleJob(id, job.id))}>
                                        {tr.plugins.removeJob}
                                    </button>
                                )}
                            </div>
                        )}
                    </div>
                ))}
            </div>
            {note && (
                <span class={`t-caption ${note.ok ? 'c-accent-text' : 'c-dot-offline'}`} role={note.ok ? 'status' : 'alert'}>
                    {note.text}
                </span>
            )}
        </WinSection>
    );
}

/** Выключатель «Расписания плагинов» (этап 3): все ритмы и задачи разом; сами задачи остаются. Видит тот, кто видит
 *  подключения, меняет администратор. */
export function SchedulesSwitchLine() {
    const panel = usePanel();
    const [state, setState] = useState<{ on: boolean; allowed: boolean } | null>(null);
    const [busy, setBusy] = useState(false);
    const [error, setError] = useState<string | null>(null);
    useEffect(() => {
        api.schedulesSwitch()
            .then(setState)
            .catch(() => setState(null));
    }, [panel.version]);
    if (!state) return null;
    if (!state.allowed) return <p class="t-caption m-0 mb-6">{tr.plugins.schedulesEnvOff}</p>;
    const flip = async (on: boolean) => {
        setBusy(true);
        setError(null);
        try {
            setState(await api.setSchedulesSwitch(on));
        } catch (e) {
            setError(problemOf(e));
        } finally {
            setBusy(false);
        }
    };
    return (
        <div class="flex flex-col gap-4 mb-6">
            <div class="pkg-switch">
                <div class="flex-1 min-w-0 flex flex-col gap-2">
                    <span class="fw-700">{tr.plugins.schedulesTitle}</span>
                    <span class="t-caption">{state.on ? tr.plugins.schedulesOn : tr.plugins.schedulesOffShort}</span>
                </div>
                <Switch on={state.on} label={tr.plugins.schedulesTitle} disabled={busy || !isAdmin()} onChange={(on) => void flip(on)} />
            </div>
            {error && (
                <span class="t-caption c-dot-offline" role="alert">
                    {error}
                </span>
            )}
        </div>
    );
}

/** Отложенные команды («через 10 минут выключи свет», этап 9): что и когда, «Снять». Нет ни одной — строки нет. */
export function LaterLine() {
    const panel = usePanel();
    const [commands, setCommands] = useState<LaterCommandView[]>([]);
    const [busy, setBusy] = useState<string | null>(null);
    const [error, setError] = useState<string | null>(null);
    const read = () =>
        api.laterCommands()
            .then(setCommands)
            .catch(() => setCommands([]));
    useEffect(() => {
        void read();
    }, [panel.version]);
    if (commands.length === 0) return null;
    const cancel = async (id: string) => {
        setBusy(id);
        setError(null);
        try {
            await api.cancelLaterCommand(id);
            await read();
        } catch (e) {
            setError(problemOf(e));
        } finally {
            setBusy(null);
        }
    };
    const when = (at: number) => new Date(at).toLocaleString(locale(), { day: '2-digit', month: '2-digit', hour: '2-digit', minute: '2-digit' });
    return (
        <div class="flex flex-col gap-4 mb-6">
            <span class="fw-700">{tr.plugins.laterTitle}</span>
            {commands.map((command) => (
                <div key={command.id} class="flex flex-wrap items-center gap-8">
                    <span class="t-caption">
                        {tr.plugins.laterItem(command.command, when(command.at))}
                    </span>
                    <Action label={tr.plugins.removeJob} busy={busy === command.id} disabled={!!busy} onClick={() => void cancel(command.id)} />
                </div>
            ))}
            {error && (
                <span class="t-caption c-dot-offline" role="alert">
                    {error}
                </span>
            )}
        </div>
    );
}

/** Раздел (окно) приложения в пульте — его открывают со страницы приложения и ярлыком; нет окна — null. */
export function appSectionOf(id: string): string | null {
    const plugin = registeredPlugins().find((entry) => entry.id === id || entry.id === `package.${id}`);
    return (plugin?.sections?.find((section) => section.nav !== 'link') ?? plugin?.sections?.[0])?.id ?? null;
}

/** Вкладки окна приложения. */
type AppTab = 'overview' | 'access' | 'settings' | 'versions';

/** Пояснение словами без ведущих « — », «: » и скобок: строки сделаны для продолжения фразы. */
const bare = (text: string): string => text.replace(/^[\s—:(]+/, '').replace(/\)$/, '');

/**
 * Окно установленного пакета (холст «Окна DuoWave», владелец 17.11): шапка — значок, имя и метки (состояние, откуда,
 * где, версия), редкое — за «⋯». Вкладки: «Обзор» — что это, что сказать, что даёт, состояние, «Опасное»; «Доступ» —
 * права, адреса, ОЗУ, картинки и связи; «Настройки» — поля, «Сохранить» снизу; «Версии» — история. Главная кнопка одна,
 * внизу справа: «Включить», «Обновить» или «Открыть».
 */
export function InstalledPackageSheet({
    entry,
    onClose,
    onChanged,
    onRemove,
    update,
}: {
    entry: PackageView;
    onClose: () => void;
    /** После действия: экран перечитывает список. */
    onChanged: () => void;
    /** Удаление — общим путём экрана пакетов (спрашивает подтверждение). */
    onRemove: (entry: PackageView) => void;
    /** В MagicPlace есть новая версия: «Обновить» — общим путём экрана пакетов. */
    update?: { busy: boolean; run: () => void };
}) {
    const panel = usePanel();
    const [view, setView] = useState<PackageView>(entry);
    const [busy, setBusy] = useState<string | null>(null);
    const [error, setError] = useState<string | null>(null);
    const [tab, setTab] = useState<AppTab>('overview');
    useEffect(() => setView(entry), [entry]);
    const kind = view.kind ?? 'plugin';
    // Приложение (своя программа рядом со шлюзом) ведёт себя как плагин: включается, работает, пишет журнал.
    const runs = kind === 'plugin' || kind === 'app';
    const origin: PackageOrigin = view.origin ?? (view.source === 'catalog' ? 'catalog' : 'user');
    const admin = isAdmin();
    const run = async (what: string, action: () => Promise<PackageView | void>) => {
        setBusy(what);
        setError(null);
        try {
            const next = await action();
            if (next) setView(next);
            onChanged();
        } catch (e) {
            setError(problemOf(e));
        } finally {
            setBusy(null);
        }
    };
    const settings = useSettingsForm(view, (next) => {
        setView(next);
        onChanged();
    });
    const memory = pluginMemoryLine(view);
    const contributes = view.contributes;
    const section = appSectionOf(view.id);
    // «Ярлык на столе» (docs/plans/menu-scheme.md): ярлыки — в раскладке стола, она на шлюзе. Переключатель — только
    // когда лента ярлыков включена в настройках.
    const [desk, deskDo] = useDeskLayout(panel.role.role);
    const shortcuts = desk.shortcuts ?? STARTING_SHORTCUTS;
    const pinned = shortcuts.includes(view.id);
    const hosts = view.permissions.filter((permission) => permission.startsWith('net:')).map((host) => host.slice(4));
    const rights = view.permissions.filter((permission) => !permission.startsWith('net:'));
    const forkable = admin && (origin === 'bundled' || origin === 'catalog') && (kind === 'plugin' || kind === 'theme');
    const editable = admin && (view.editable ?? origin === 'user') && (kind === 'plugin' || kind === 'theme');
    const canDisable = admin && runs && view.enabled && (view.canDisable ?? true);
    const removable = admin && (view.removable ?? origin !== 'bundled') && origin !== 'system';
    const open = section && view.enabled ? () => (onClose(), panel.go(section)) : null;

    const tabs: Array<{ id: AppTab; label: string }> = [
        { id: 'overview', label: tr.plugins.tabOverview },
        ...(runs || view.permissions.length > 0 ? [{ id: 'access' as const, label: tr.plugins.tabAccess }] : []),
        ...(admin && runs && settings.has ? [{ id: 'settings' as const, label: tr.plugins.tabSettings }] : []),
        ...(admin && origin !== 'system' ? [{ id: 'versions' as const, label: tr.plugins.tabVersions }] : []),
    ];
    const shown = tabs.some((entry) => entry.id === tab) ? tab : 'overview';

    const menu = [
        ...(runs && view.enabled && admin ? [{ label: tr.plugins.reload, disabled: !!busy, act: () => void run('reload', () => api.reloadPackage(view.id)) }] : []),
        ...(editable ? [{ label: tr.plugins.openWorkshop, act: () => (onClose(), panel.go('skills', 'workshop', view.id)) }] : []),
        ...(forkable
            ? [
                  {
                      label: tr.plugins.fork,
                      disabled: !!busy,
                      act: () =>
                          void run('fork', async () => {
                              const copy = await api.forkPackage(view.id);
                              onClose();
                              panel.go('skills', 'workshop', copy.id);
                          }),
                  },
              ]
            : []),
    ];

    const state = !runs ? null : view.problem ? (
        <span class="pill pill-error">{tr.plugins.stateProblem}</span>
    ) : view.enabled ? (
        <span class="pill pill-own">{tr.plugins.stateOn}</span>
    ) : (
        <span class="pill pill-off">{tr.plugins.stateOff}</span>
    );
    const meta = (
        <>
            {state}
            <span class="win-chip">{ORIGIN_TITLES[origin]}</span>
            <span class="win-chip">{placeOf(kind, view.runtime, view.runs)}</span>
            {view.version && <span class="win-chip">{view.version}</span>}
        </>
    );

    // Нижняя полоса: на «Настройках» — «Сохранить», когда изменили; иначе — одно главное действие.
    const footer =
        shown === 'settings' ? (
            settings.dirty || settings.busy ? (
                <SaveBar dirty={settings.dirty} busy={settings.busy} saveLabel={saveLabel()} onRevert={settings.revert} onSave={() => void settings.save()} />
            ) : undefined
        ) : admin && runs && !view.enabled && (view.canDisable ?? true) ? (
            <button class="btn btn-main" disabled={!!busy} aria-busy={busy === 'toggle' || undefined} onClick={() => void run('toggle', () => api.enablePackage(view.id))}>
                {busy === 'toggle' && <span class="spinner spinner-sm" aria-hidden="true" />}
                {tr.plugins.turnOn}
            </button>
        ) : update ? (
            <>
                {open && (
                    <button class="btn btn-plain" onClick={open}>
                        {tr.magicplace.open}
                    </button>
                )}
                <button class="btn btn-main" disabled={update.busy} aria-busy={update.busy || undefined} onClick={update.run}>
                    {update.busy && <span class="spinner spinner-sm" aria-hidden="true" />}
                    {tr.catalog.update}
                </button>
            </>
        ) : open ? (
            <button class="btn btn-main" onClick={open}>
                {tr.magicplace.open}
            </button>
        ) : undefined;

    // Фразы как их говорят: «какая погода {when}» → «какая погода …»; одинаковые после этого — одной меткой.
    const phrases = [...new Set((contributes?.phrases ?? []).map((phrase) => phrase.replace(/\{[^}]+\}/g, '…')))];
    const gives = contributes && runs && (contributes.tools.length > 0 || contributes.media.length > 0 || contributes.events.length > 0 || contributes.panel);
    const panelWords = !contributes?.panel
        ? ''
        : view.panelModule
          ? origin === 'system' || origin === 'bundled'
              ? ''
              : bare(tr.plugins.panelOwnWindow)
          : view.enabled
            ? bare(tr.plugins.panelCards)
            : bare(tr.plugins.panelWhenOn);
    const status = runs && (view.lastCallAt || view.problem || view.lastError);
    const where = view.runs.length > 0 ? view.runs.map((place) => ({ pc: tr.plugins.runsPc, station: tr.plugins.runsStation, stationServer: tr.plugins.runsStationServer })[place] ?? place).join(', ') : null;

    return (
        <Sheet
            title={view.title}
            icon={<AppIcon id={view.id} title={view.title} size={44} />}
            meta={meta}
            menu={menu}
            tabs={tabs}
            tab={shown}
            onTab={setTab}
            onClose={onClose}
            footer={footer}
        >
            {error && (
                <p class="win-callout m-0" role="alert">
                    {error}
                </p>
            )}
            {shown === 'overview' && (
                <>
                    {view.description && <p class="win-lead">{view.description}</p>}
                    {admin && runs && !view.enabled && (view.canDisable ?? true) && <p class="win-callout is-calm m-0">{tr.plugins.offNote}</p>}
                    {runs && phrases.length > 0 && (
                        <WinSection title={tr.plugins.sayTitle}>
                            <div class="win-tags">
                                {phrases.slice(0, 8).map((phrase) => (
                                    <span key={phrase} class="win-tag">
                                        {phrase}
                                    </span>
                                ))}
                                {phrases.length > 8 && <span class="win-tag is-more">{tr.plugins.phrasesMore(phrases.length - 8)}</span>}
                            </div>
                        </WinSection>
                    )}
                    {(gives || (runs && view.enabled && desk.shortcutsBar)) && (
                        <WinSection title={tr.plugins.contributes} group>
                            {contributes && runs && contributes.tools.length > 0 && (
                                <div class="win-row is-col">
                                    <div class="win-row-main">
                                        <span class="win-row-t">{tr.plugins.toolsTitle}</span>
                                        <span class="win-row-s">{tr.plugins.toolsAbout}</span>
                                    </div>
                                    <div class="win-tags">
                                        {contributes.tools.map((tool) => (
                                            <span key={tool} class="win-code">
                                                {tool}
                                            </span>
                                        ))}
                                    </div>
                                </div>
                            )}
                            {contributes && runs && contributes.media.length > 0 && (
                                <WinRow title={tr.plugins.sourcesTitle} sub={contributes.media.join(', ')} />
                            )}
                            {contributes && runs && contributes.events.length > 0 && (
                                <div class="win-row is-col">
                                    <span class="win-row-t">{tr.plugins.eventsTitle}</span>
                                    <div class="win-tags">
                                        {contributes.events.map((event) => (
                                            <span key={event} class="win-code">
                                                {event}
                                            </span>
                                        ))}
                                    </div>
                                </div>
                            )}
                            {contributes?.panel && runs && <WinRow title={tr.plugins.panelPart} sub={panelWords || undefined} />}
                            {runs && view.enabled && desk.shortcutsBar && (
                                <WinRow title={tr.desk.shortcutOnDesk} sub={tr.desk.shortcutOnDeskAbout}>
                                    <Switch
                                        on={pinned}
                                        label={tr.desk.shortcutOnDesk}
                                        onChange={(on) => deskDo.shortcuts(on ? [...shortcuts.filter((id) => id !== view.id), view.id] : shortcuts.filter((id) => id !== view.id))}
                                    />
                                </WinRow>
                            )}
                        </WinSection>
                    )}
                    {runs && view.permissions.includes('call:system/schedule') && <ScheduleBlock id={view.id} admin={admin} />}
                    {status && (
                        <WinSection title={tr.plugins.statusTitle} group>
                            {view.problem && (
                                <div class="win-row">
                                    <span class="win-row-s c-dot-offline" role="alert">
                                        {view.problem}
                                    </span>
                                </div>
                            )}
                            {view.lastError && (
                                <div class="win-row">
                                    <span class="win-row-s c-dot-offline">{tr.plugins.lastError(timeAgo(view.lastError.at), view.lastError.message)}</span>
                                </div>
                            )}
                            {view.lastCallAt && (
                                <div class="win-row">
                                    <span class="win-row-s">{tr.plugins.lastCall(timeAgo(view.lastCallAt))}</span>
                                </div>
                            )}
                        </WinSection>
                    )}
                    {(where || view.runsAt) && (
                        <WinSection title={tr.plugins.whereTitle} group>
                            <WinRow
                                title={where ?? ''}
                                sub={view.runsAt === 'home' ? tr.plugins.runsAtHome : view.runsAt === 'here' ? tr.plugins.runsAtHere : undefined}
                            />
                        </WinSection>
                    )}
                    {(canDisable || removable) && (
                        <WinSection title={tr.ui.danger} group>
                            {canDisable && (
                                <WinRow title={tr.plugins.turnOff} sub={tr.plugins.turnOffAbout}>
                                    <button class="btn btn-danger btn-sm" disabled={!!busy} aria-busy={busy === 'toggle' || undefined} onClick={() => void run('toggle', () => api.disablePackage(view.id))}>
                                        {tr.plugins.turnOff}
                                    </button>
                                </WinRow>
                            )}
                            {removable && (
                                <WinRow title={tr.plugins.remove} sub={tr.plugins.removeAbout}>
                                    <button class="btn btn-danger btn-sm" disabled={!!busy} onClick={() => onRemove(view)}>
                                        {tr.plugins.remove}
                                    </button>
                                </WinRow>
                            )}
                        </WinSection>
                    )}
                    <p class="win-note">{ORIGIN_RULES[origin]}</p>
                    {!admin && runs && <p class="win-note">{tr.plugins.adminOnly}</p>}
                </>
            )}
            {shown === 'access' && (
                <>
                    <WinSection title={tr.plugins.takesTitle} group>
                        {rights.map((right) => (
                            <WinRow key={right} title={permissionTitle(right)}>
                                <span class="win-code">{right}</span>
                            </WinRow>
                        ))}
                        {hosts.length > 0 && (
                            <div class="win-row is-col">
                                <div class="win-row-main">
                                    <span class="win-row-t">{tr.plugins.internetTitle}</span>
                                    <span class="win-row-s">{tr.plugins.internetAbout}</span>
                                </div>
                                <div class="win-tags">
                                    {hosts.map((host) => (
                                        <span key={host} class="win-code">
                                            {host}
                                        </span>
                                    ))}
                                </div>
                            </div>
                        )}
                        {memory && runs && <WinRow title={memory} />}
                        {runs && <FilesLine id={view.id} />}
                        {runs && view.permissions.includes('listen:device/midi') && <MidiLine id={view.id} />}
                        {view.permissions.length === 0 && !memory && <WinRow title={tr.ui.rightsNoneHome} />}
                    </WinSection>
                    {admin && runs && view.enabled && <LinksBlock id={view.id} />}
                </>
            )}
            {shown === 'settings' && settings.form}
            {shown === 'versions' && admin && (
                <HistoryCard
                    id={view.id}
                    mode="installed"
                    onChanged={() =>
                        void api.packages().then((all) => {
                            const next = all.packages.find((found) => found.id === view.id);
                            if (next) setView(next);
                            onChanged();
                        })
                    }
                />
            )}
        </Sheet>
    );
}
