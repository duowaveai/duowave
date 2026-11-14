// Окно установленного пакета (docs/plans/plugins.md, «Экран «Плагины» и три вида пакетов», этап 5): что с ним сейчас
// (работает, ошибка, выключен), его память против бюджета, вызовы и последняя ошибка, права и хосты, что он даёт
// шлюзу, его настройки — и кнопки по виду: системный не выключить и не удалить, встроенный — только выключить или
// сделать свою копию, свой — всё, включая «Открыть в мастерской». Шлюз проверяет каждое действие сам.

import { useEffect, useState } from 'preact/hooks';

import { LaterCommandView, PackageOrigin, PackageView, MidiView, PluginFilesView, ScheduleJob, ScheduleSpec, ScheduleView, api, problemOf, sessionStore, can } from '../api';
import { formatNumber, locale, tr } from '../i18n';
import { usePanel } from '../shell/context';
import { timeAgo } from '../time';
import { Action } from '../ui/cards';
import { Sheet } from '../ui/sheet';
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
        <div class="flex flex-col gap-4">
            <span class="t-caption">
                {tr.plugins.filesLine(view.files.length, mb(view.bytes), view.onStation ? null : mb(view.limitBytes))}
            </span>
            {view.onStation && (
                <span class="t-caption">
                    {tr.plugins.filesStation(mb(view.limitBytes), allBytes != null ? mb(allBytes) : null)}
                </span>
            )}
        </div>
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
    if (!view.available) return <span class="t-caption">{tr.plugins.midiUnavailable(view.why ?? tr.plugins.midiNone)}</span>;
    const listening = view.listeners.filter((listener) => listener.package === id);
    const where = (zone: string): string => (zone === 'local' ? tr.plugins.zoneLocal : zone === 'home' ? tr.plugins.zoneHome : tr.plugins.zoneAway);
    return (
        <span class="t-caption">
            {listening.length > 0
                ? tr.plugins.midiListening(
                      [...new Set(listening.flatMap((listener) => listener.devices))].join(', '),
                      [...new Set(listening.map((listener) => where(listener.zone)))].join(', '),
                  )
                : view.devices.length > 0
                  ? tr.plugins.midiIdle(view.devices.map((device) => device.name).join(', '))
                  : tr.plugins.midiNotConnected}
        </span>
    );
}

/** Право словами — одна функция на весь пульт (bus/rights.ts). */
export const permissionTitle = rightTitle;

const isAdmin = (): boolean => can('plugins.manage') && sessionStore.get()?.role === 'admin';

/** Поля настроек плагина: строка (или выбор), число, да/нет, секрет — форма из манифеста. */
function SettingsForm({ entry, onSaved }: { entry: PackageView; onSaved: (view: PackageView) => void }) {
    const fields = entry.settings ?? [];
    const [values, setValues] = useState<Record<string, unknown>>(() => Object.fromEntries(fields.map((field) => [field.key, field.value ?? field.default ?? null])));
    const [busy, setBusy] = useState(false);
    const [note, setNote] = useState<{ ok: boolean; text: string } | null>(null);
    // Пока открыто поле личного, микрофон пульта выключен: ключ вслух не называют.
    const [privateOpen, setPrivateOpen] = useState(false);
    useMicHold(privateOpen);
    useEffect(() => setValues(Object.fromEntries(fields.map((field) => [field.key, field.value ?? field.default ?? null]))), [entry.id, entry.version]);
    if (fields.length === 0) return null;
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
    return (
        <div class="flex flex-col gap-8">
            <h3 class="t-title-xs m-0">{tr.plugins.settings}</h3>
            <SettingsFields fields={fields} values={values} busy={busy} onChange={setValues} onPrivate={setPrivateOpen} />
            <div>
                <Action tone="on" label={saveLabel()} busy={busy} onClick={() => void save()} />
            </div>
            {note && (
                <span class={`t-caption ${note.ok ? 'c-accent-text' : 'c-dot-offline'}`} role={note.ok ? 'status' : 'alert'}>
                    {note.text}
                </span>
            )}
        </div>
    );
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
    const [view, setView] = useState<ScheduleView | null>(null);
    const [busy, setBusy] = useState<string | null>(null);
    const [note, setNote] = useState<{ ok: boolean; text: string } | null>(null);
    useEffect(() => {
        let alive = true;
        api.packageSchedule(id)
            .then((answer) => alive && setView(answer))
            .catch(() => alive && setView(null));
        return () => {
            alive = false;
        };
    }, [id, panel.version]);
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
        <div class="flex flex-col gap-8">
            <h3 class="t-title-xs m-0">{tr.plugins.schedule}</h3>
            {!view.on && <span class="t-caption c-pill-yandex-fg">{tr.plugins.schedulesOff}</span>}
            {view.jobs.map((job) => (
                <div key={job.id} class="flex flex-col gap-4">
                    <span class="fw-700">{job.title}</span>
                    <span class="t-caption">{line(job)}</span>
                    {job.problem && (
                        <span class="t-caption c-dot-offline" role="alert">
                            {job.problem}
                        </span>
                    )}
                    {admin && (
                        <div class="flex flex-wrap items-center gap-8">
                            <Action label={tr.plugins.runNow} busy={busy === `run:${job.id}`} disabled={!!busy} onClick={() => void act(`run:${job.id}`, () => api.runScheduleJob(id, job.id))} />
                            <Action
                                label={job.paused ? tr.plugins.resume : tr.plugins.pause}
                                busy={busy === `pause:${job.id}`}
                                disabled={!!busy}
                                onClick={() => void act(`pause:${job.id}`, () => api.pauseScheduleJob(id, job.id, !job.paused))}
                            />
                            {job.source === 'code' && (
                                <Action label={tr.plugins.removeJob} busy={busy === `remove:${job.id}`} disabled={!!busy} onClick={() => void act(`remove:${job.id}`, () => api.removeScheduleJob(id, job.id))} />
                            )}
                        </div>
                    )}
                </div>
            ))}
            {note && (
                <span class={`t-caption ${note.ok ? 'c-accent-text' : 'c-dot-offline'}`} role={note.ok ? 'status' : 'alert'}>
                    {note.text}
                </span>
            )}
        </div>
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

/** Окно установленного пакета: состояние, память, права, что даёт, настройки и кнопки по его виду. */
export function InstalledPackageSheet({
    entry,
    onClose,
    onChanged,
    onRemove,
}: {
    entry: PackageView;
    onClose: () => void;
    /** После действия: экран перечитывает список. */
    onChanged: () => void;
    /** Удаление — общим путём экрана пакетов (спрашивает подтверждение). */
    onRemove: (entry: PackageView) => void;
}) {
    const panel = usePanel();
    const [view, setView] = useState<PackageView>(entry);
    const [busy, setBusy] = useState<string | null>(null);
    const [error, setError] = useState<string | null>(null);
    /** «История и версии»: лента приложения в этом окне. */
    const [history, setHistory] = useState(false);
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
    const memory = pluginMemoryLine(view);
    const contributes = view.contributes;
    const hosts = view.permissions.filter((permission) => permission.startsWith('net:'));
    const rights = view.permissions.filter((permission) => !permission.startsWith('net:'));
    const forkable = admin && (origin === 'bundled' || origin === 'catalog') && (kind === 'plugin' || kind === 'theme');
    const editable = admin && (view.editable ?? origin === 'user') && (kind === 'plugin' || kind === 'theme');
    return (
        <Sheet title={view.title} about={`${ORIGIN_TITLES[origin]} · ${placeOf(kind, view.runtime)}${view.version ? tr.plugins.versionSuffix(view.version) : ''}`} onClose={onClose}>
            {view.description && <p class="t-body-sm m-0">{view.description}</p>}
            <p class="t-caption m-0">{ORIGIN_RULES[origin]}</p>
            {runs && (
                <div class="flex flex-col gap-4">
                    {memory && <span class="t-caption">{memory}</span>}
                    {view.lastCallAt && <span class="t-caption">{tr.plugins.lastCall(timeAgo(view.lastCallAt))}</span>}
                    {view.problem && (
                        <span class="t-caption c-dot-offline" role="alert">
                            {view.problem}
                        </span>
                    )}
                    {view.lastError && (
                        <span class="t-caption c-dot-offline">
                            {tr.plugins.lastError(timeAgo(view.lastError.at), view.lastError.message)}
                        </span>
                    )}
                </div>
            )}
            {contributes && runs && (
                <div class="flex flex-col gap-4">
                    <h3 class="t-title-xs m-0">{tr.plugins.contributes}</h3>
                    {contributes.phrases.length > 0 && <span class="t-caption">{tr.plugins.phrases(contributes.phrases.slice(0, 6), Math.max(0, contributes.phrases.length - 6))}</span>}
                    {contributes.tools.length > 0 && <span class="t-caption">{tr.plugins.brainTools(contributes.tools.join(', '))}</span>}
                    {contributes.media.length > 0 && <span class="t-caption">{tr.plugins.sources(contributes.media.join(', '))}</span>}
                    {contributes.events.length > 0 && <span class="t-caption">{tr.plugins.events(contributes.events.join(', '))}</span>}
                    {contributes.panel && (
                        <span class="t-caption">
                            {tr.plugins.panelPart}
                            {view.panelModule
                                ? origin === 'system' || origin === 'bundled'
                                    ? ''
                                    : tr.plugins.panelOwnWindow
                                : view.enabled
                                  ? tr.plugins.panelCards
                                  : tr.plugins.panelWhenOn}
                        </span>
                    )}
                </div>
            )}
            {view.permissions.length > 0 && (
                <div class="flex flex-col gap-4">
                    <h3 class="t-title-xs m-0">{tr.plugins.rights}</h3>
                    {rights.length > 0 && <span class="t-caption">{rights.map(permissionTitle).join(', ')}</span>}
                    {hosts.length > 0 && <span class="t-caption">{tr.plugins.network(hosts.map((host) => host.slice(4)).join(', '))}</span>}
                </div>
            )}
            {view.runsAt === 'home' && <span class="t-caption">{tr.plugins.runsAtHome}</span>}
            {view.runsAt === 'here' && <span class="t-caption">{tr.plugins.runsAtHere}</span>}
            {view.runs.length > 0 && <span class="t-caption">{tr.plugins.runsList(view.runs.map((runs) => ({ pc: tr.plugins.runsPc, station: tr.plugins.runsStation, stationServer: tr.plugins.runsStationServer })[runs] ?? runs).join(', '))}</span>}
            {runs && view.permissions.includes('call:system/schedule') && <ScheduleBlock id={view.id} admin={admin} />}
            {runs && <FilesLine id={view.id} />}
            {runs && view.permissions.includes('listen:device/midi') && <MidiLine id={view.id} />}
            {admin && runs && view.enabled && <LinksBlock id={view.id} />}
            {admin && runs && <SettingsForm entry={view} onSaved={(next) => { setView(next); onChanged(); }} />}
            {admin && (
                <div class="flex flex-wrap items-center gap-8">
                    {runs && (view.canDisable ?? true) && (
                        <Action
                            tone={view.enabled ? 'off' : 'on'}
                            label={view.enabled ? tr.plugins.turnOff : tr.plugins.turnOn}
                            busy={busy === 'toggle'}
                            disabled={!!busy}
                            onClick={() => void run('toggle', () => (view.enabled ? api.disablePackage(view.id) : api.enablePackage(view.id)))}
                        />
                    )}
                    {runs && view.enabled && (
                        <Action label={tr.plugins.reload} busy={busy === 'reload'} disabled={!!busy} onClick={() => void run('reload', () => api.reloadPackage(view.id))} />
                    )}
                    {origin !== 'system' && (
                        <Action label={history ? tr.plugins.hideHistory : tr.plugins.history} disabled={!!busy} onClick={() => setHistory((open) => !open)} />
                    )}
                    {editable && (
                        <Action
                            label={tr.plugins.openWorkshop}
                            disabled={!!busy}
                            onClick={() => {
                                onClose();
                                panel.go('skills', 'workshop', view.id);
                            }}
                        />
                    )}
                    {forkable && (
                        <Action
                            label={tr.plugins.fork}
                            busy={busy === 'fork'}
                            disabled={!!busy}
                            onClick={() =>
                                void run('fork', async () => {
                                    const copy = await api.forkPackage(view.id);
                                    onClose();
                                    panel.go('skills', 'workshop', copy.id);
                                })
                            }
                        />
                    )}
                    {(view.removable ?? origin !== 'bundled') && origin !== 'system' && (
                        <Action label={tr.plugins.remove} disabled={!!busy} onClick={() => onRemove(view)} />
                    )}
                </div>
            )}
            {admin && history && (
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
            {!admin && runs && <p class="t-caption m-0">{tr.plugins.adminOnly}</p>}
            {error && (
                <p class="t-caption m-0 c-dot-offline" role="alert">
                    {error}
                </p>
            )}
        </Sheet>
    );
}
