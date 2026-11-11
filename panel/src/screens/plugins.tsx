// Окно установленного пакета (docs/plans/plugins.md, «Экран «Плагины» и три вида пакетов», этап 5): что с ним сейчас
// (работает, ошибка, выключен), его память против бюджета, вызовы и последняя ошибка, права и хосты, что он даёт
// шлюзу, его настройки — и кнопки по виду: системный не выключить и не удалить, встроенный — только выключить или
// сделать свою копию, свой — всё, включая «Открыть в мастерской». Шлюз проверяет каждое действие сам.

import { useEffect, useState } from 'preact/hooks';

import { PackageOrigin, PackageView, api, problemOf, sessionStore, can } from '../api';
import { usePanel } from '../shell/context';
import { timeAgo } from '../time';
import { Action } from '../ui/cards';
import { Sheet } from '../ui/sheet';
import { Switch } from '../ui/switch';
import { pluginMemoryLine } from './memory';
import { ask } from '../ui/confirm';
import { saveLabel } from '../via';

/** Вид пакета по тому, откуда он, — на метке и в окне. */
export const ORIGIN_TITLES: Record<PackageOrigin, string> = {
    system: 'системный',
    bundled: 'встроенный',
    user: 'свой',
    catalog: 'из каталога',
};

const ORIGIN_RULES: Record<PackageOrigin, string> = {
    system: 'Системный: в сборке шлюза, без него колонка не работает как надо — его не выключить и не удалить.',
    bundled: 'Встроенный: в сборке шлюза и обновляется вместе с ним. Его можно выключить; чтобы менять — сделайте свою копию.',
    user: 'Свой: написан в мастерской или поставлен руками. С ним можно всё — менять, выключать, удалять.',
    catalog: 'Из каталога: обновляется из каталога. Его можно выключить и удалить; чтобы менять — сделайте свою копию.',
};

const PERMISSION_TITLES: Record<string, string> = {
    'voice.intents': 'фразы колонки',
    'voice.say': 'говорить без запроса',
    'llm.tools': 'инструменты «Мозга»',
    'media.source': 'источник музыки, радио и сказок',
    kv: 'своё хранилище',
    events: 'события шлюза',
};

/** Право плагина словами: «сеть: *.api.radio-browser.info», «фразы колонки». */
export function permissionTitle(permission: string): string {
    if (permission.startsWith('net:')) return `сеть: ${permission.slice(4)}`;
    return PERMISSION_TITLES[permission] ?? permission;
}

const isAdmin = (): boolean => can('plugins.manage') && sessionStore.get()?.role === 'admin';

/** Маска секрета (ссылка с ключом внутри): значение шлюз не отдаёт, а та же маска при сохранении оставляет прежнее. */
const SECRET_MASK = '••••••';

type SettingField = NonNullable<PackageView['settings']>[number];

const isSecret = (field: SettingField): boolean => field.type === 'secret' || field.secret === true || field.value === SECRET_MASK;

/** Поля настроек плагина: строка (или выбор), число, да/нет, секрет — форма из манифеста. */
function SettingsForm({ entry, onSaved }: { entry: PackageView; onSaved: (view: PackageView) => void }) {
    const fields = entry.settings ?? [];
    const [values, setValues] = useState<Record<string, unknown>>(() => Object.fromEntries(fields.map((field) => [field.key, field.value ?? field.default ?? null])));
    const [busy, setBusy] = useState(false);
    const [note, setNote] = useState<{ ok: boolean; text: string } | null>(null);
    useEffect(() => setValues(Object.fromEntries(fields.map((field) => [field.key, field.value ?? field.default ?? null]))), [entry.id, entry.version]);
    if (fields.length === 0) return null;
    const save = async () => {
        setBusy(true);
        setNote(null);
        try {
            onSaved(await api.setPackageSettings(entry.id, values));
            setNote({ ok: true, text: 'Сохранено.' });
        } catch (e) {
            setNote({ ok: false, text: problemOf(e) });
        } finally {
            setBusy(false);
        }
    };
    return (
        <div class="flex flex-col gap-8">
            <h3 class="t-title-xs m-0">Настройки</h3>
            {fields.map((field) => (
                <label key={field.key} class="flex flex-col gap-4">
                    <span class="fw-700">{field.title}</span>
                    {field.description && <span class="t-caption">{field.description}</span>}
                    {field.type === 'boolean' ? (
                        <Switch on={values[field.key] === true} label={field.title} disabled={busy} onChange={(on) => setValues({ ...values, [field.key]: on })} />
                    ) : isSecret(field) ? (
                        // Заданный секрет не показывается: поле пустое, и пустым оно уходит маской — шлюз оставит прежнее.
                        <input
                            class="field"
                            type="password"
                            autocomplete="off"
                            spellcheck={false}
                            value={values[field.key] === SECRET_MASK ? '' : String(values[field.key] ?? '')}
                            placeholder={field.value === SECRET_MASK ? 'задано · впишите новое, чтобы заменить' : 'вставьте сюда'}
                            disabled={busy}
                            onInput={(e) => {
                                const text = (e.target as HTMLInputElement).value;
                                setValues({ ...values, [field.key]: text === '' && field.value === SECRET_MASK ? SECRET_MASK : text });
                            }}
                        />
                    ) : field.options ? (
                        <select class="field" value={String(values[field.key] ?? '')} disabled={busy} onChange={(e) => setValues({ ...values, [field.key]: (e.target as HTMLSelectElement).value })}>
                            {field.options.map((option) => (
                                <option key={option} value={option}>
                                    {option}
                                </option>
                            ))}
                        </select>
                    ) : (
                        <input
                            class="field"
                            type={field.type === 'number' ? 'number' : 'text'}
                            value={values[field.key] == null ? '' : String(values[field.key])}
                            disabled={busy}
                            onInput={(e) => {
                                const text = (e.target as HTMLInputElement).value;
                                setValues({ ...values, [field.key]: field.type === 'number' ? (text === '' ? null : Number(text)) : text });
                            }}
                        />
                    )}
                </label>
            ))}
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
        <Sheet title={view.title} about={`${ORIGIN_TITLES[origin]} · ${kind === 'plugin' ? 'плагин' : kind === 'app' ? 'приложение' : kind}${view.version ? ` · версия ${view.version}` : ''}`} onClose={onClose}>
            {view.description && <p class="t-body-sm m-0">{view.description}</p>}
            <p class="t-caption m-0">{ORIGIN_RULES[origin]}</p>
            {runs && (
                <div class="flex flex-col gap-4">
                    {memory && <span class="t-caption">{memory}</span>}
                    {view.lastCallAt && <span class="t-caption">Последний вызов {timeAgo(view.lastCallAt)}.</span>}
                    {view.problem && (
                        <span class="t-caption c-dot-offline" role="alert">
                            {view.problem}
                        </span>
                    )}
                    {view.lastError && (
                        <span class="t-caption c-dot-offline">
                            Последняя ошибка {timeAgo(view.lastError.at)}: {view.lastError.message}
                        </span>
                    )}
                </div>
            )}
            {contributes && runs && (
                <div class="flex flex-col gap-4">
                    <h3 class="t-title-xs m-0">Что даёт</h3>
                    {contributes.phrases.length > 0 && <span class="t-caption">Фразы: {contributes.phrases.slice(0, 6).map((phrase) => `«${phrase}»`).join(', ')}{contributes.phrases.length > 6 ? ` и ещё ${contributes.phrases.length - 6}` : ''}</span>}
                    {contributes.tools.length > 0 && <span class="t-caption">Инструменты «Мозга»: {contributes.tools.join(', ')}</span>}
                    {contributes.media.length > 0 && <span class="t-caption">Источники: {contributes.media.join(', ')}</span>}
                    {contributes.events.length > 0 && <span class="t-caption">События: {contributes.events.join(', ')}</span>}
                    {contributes.panel && <span class="t-caption">Своя часть пульта{view.panelModule ? '' : ' (загрузится, когда плагин включён)'}</span>}
                </div>
            )}
            {view.permissions.length > 0 && (
                <div class="flex flex-col gap-4">
                    <h3 class="t-title-xs m-0">Права</h3>
                    {rights.length > 0 && <span class="t-caption">{rights.map(permissionTitle).join(', ')}</span>}
                    {hosts.length > 0 && <span class="t-caption">Ходит в сеть: {hosts.map((host) => host.slice(4)).join(', ')}</span>}
                </div>
            )}
            {view.runsAt === 'home' && <span class="t-caption">Тяжёлое — на компьютере: пока он на связи, плагин работает там.</span>}
            {view.runsAt === 'here' && <span class="t-caption">Компьютер не на связи — плагин работает на колонке, если хватает памяти.</span>}
            {view.runs.length > 0 && <span class="t-caption">Работает: {view.runs.map((runs) => ({ pc: 'на компьютере', station: 'на колонке', stationServer: 'колонка + сервер' })[runs] ?? runs).join(', ')}</span>}
            {admin && runs && <SettingsForm entry={view} onSaved={(next) => { setView(next); onChanged(); }} />}
            {admin && (
                <div class="flex flex-wrap items-center gap-8">
                    {runs && (view.canDisable ?? true) && (
                        <Action
                            tone={view.enabled ? 'off' : 'on'}
                            label={view.enabled ? 'Выключить' : 'Включить'}
                            busy={busy === 'toggle'}
                            disabled={!!busy}
                            onClick={() => void run('toggle', () => (view.enabled ? api.disablePackage(view.id) : api.enablePackage(view.id)))}
                        />
                    )}
                    {runs && view.enabled && (
                        <Action label="Перезагрузить" busy={busy === 'reload'} disabled={!!busy} onClick={() => void run('reload', () => api.reloadPackage(view.id))} />
                    )}
                    {(view.removable ?? false) && view.versions.filter((version) => version.problems.length === 0).length > 1 && (
                        <Action
                            label={`Откатить к ${view.versions[1]?.version ?? 'прошлой'}`}
                            busy={busy === 'rollback'}
                            disabled={!!busy}
                            onClick={async () => {
                                const yes = await ask({
                                    title: `Вернуть версию ${view.versions[1]?.version}?`,
                                    text: `Версия ${view.versions[0]?.version} уйдёт.`,
                                    ok: 'Вернуть',
                                });
                                if (yes) void run('rollback', () => api.rollbackPackage(view.id));
                            }}
                        />
                    )}
                    {editable && (
                        <Action
                            label="Открыть в мастерской"
                            disabled={!!busy}
                            onClick={() => {
                                onClose();
                                panel.go('skills', 'workshop', view.id);
                            }}
                        />
                    )}
                    {forkable && (
                        <Action
                            label="Сделать свою копию"
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
                        <Action label="Удалить" disabled={!!busy} onClick={() => onRemove(view)} />
                    )}
                </div>
            )}
            {!admin && runs && <p class="t-caption m-0">Включать, настраивать и менять плагины может администратор.</p>}
            {error && (
                <p class="t-caption m-0 c-dot-offline" role="alert">
                    {error}
                </p>
            )}
        </Sheet>
    );
}
