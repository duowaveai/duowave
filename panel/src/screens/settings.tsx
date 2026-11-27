import { useEffect, useState } from 'preact/hooks';

import { AccessCatalog, ApiKey, AuditEntry, Person, Role, api, can, problemOf, sessionStore } from '../api';
import { IconJournal, IconKey, IconPalette, IconPlus } from '../icons';
import { timeAgo } from '../time';
import { ThemeCard } from '../shell/theme-picker';
import { MoreMenu, Sheet, SkeletonRows, WinSection } from '../ui/sheet';
import { SectionTabs, TabEntry } from '../ui/tabs';
import { ask } from '../ui/confirm';
import { BusCard } from '../bus/settings-card';
import { byMode } from '../words';
import { tr } from '../i18n';
import { LanguageCard } from '../i18n/picker';
import { usePanel } from '../shell/context';
import { useDeskLayout } from '../shell/widgets';
import { Switch } from '../ui/switch';
import { useVoiceAction } from '../voice/phrases';
import { VIA } from '../via';
import { openOnboarding } from './onboarding';
import { PermissionRows, usePermissions } from '../ui/permissions';

export type SettingsSectionTab = 'access' | 'look';

/**
 * «Настройки» — раздел «Ещё» рядом с «Домом», «Голосом» и «Журналом». «Доступ» — люди и их права, ключи для скриптов
 * и агентов, свой вход и история изменений; вкладка видна тому, у кого есть право хоть на что-то в ней, шлюз всё
 * равно проверяет каждое действие сам. «Оформление» — как пульт выглядит в этом браузере (тема и режим); сюда же
 * пойдут и остальные настройки пульта.
 */
export function SettingsScreen({
    catalog,
    tab,
    onTab,
    onSignOut,
    bare = false,
}: {
    catalog: AccessCatalog | null;
    tab: string;
    onTab: (tab: string) => void;
    onSignOut: () => void;
    /** Без своих вкладок: страница в списке «Настроек» (docs/plans/panel-rebuild.md). */
    bare?: boolean;
}) {
    const tabs: Array<TabEntry<string>> = [
        { id: 'access', label: tr.settings.tabAccess, icon: <IconKey size={16} /> },
        { id: 'look', label: tr.settings.tabLook, icon: <IconPalette size={16} /> },
    ];
    const shown = tabs.some((entry) => entry.id === tab) ? tab : tabs[0].id;
    return (
        <>
            {!bare && <SectionTabs label={tr.settings.title} tabs={tabs} tab={shown} onPick={onTab} />}
            {shown === 'access' && (
                <>
                    {can('users.manage') && <People catalog={catalog} />}
                    {can('users.manage') && <Keys catalog={catalog} />}
                    {can('plugins.manage') && <BusCard />}
                    <Account onSignOut={onSignOut} />
                    {/* Пульт колонки через компьютер: разрешения — этого компьютера, не колонки. */}
                    {!VIA && <PermissionsCard />}
                    {can('audit.view') && <History />}
                </>
            )}
            {shown === 'look' && (
                <>
                    <LanguageCard />
                    <ThemeCard />
                    <DeskCard />
                    {/* Первые шаги открывает только свой пульт шлюза (app.tsx): в пульте колонки через компьютер кнопке
                        нечего открыть. */}
                    {can('settings.edit') && !VIA && <OnboardingCard />}
                </>
            )}
        </>
    );
}

/**
 * «Разрешения этого компьютера» (docs/plans/permissions.md): что система должна разрешить приложению DuoWave, с кнопкой у
 * каждой строки. Только в приложении на компьютере: у сервера, колонки и Docker список пуст, и карточки нет.
 */
function PermissionsCard() {
    const perms = usePermissions();
    const view = perms.view;
    if (!view?.app || view.items.length === 0) return null;
    return (
        <section class="card glass card-list" aria-labelledby="perm-h">
            <div class="section-head">
                <h2 id="perm-h" class="t-title-xs m-0">
                    {tr.permissions.sectionTitle}
                </h2>
            </div>
            <p class="w-cap m-0">{tr.permissions.sectionAbout}</p>
            <PermissionRows perms={perms} />
        </section>
    );
}

/**
 * Стол: лента ярлыков (screens/shortcuts.tsx). По умолчанию её нет (владелец, 16.11) — включают только здесь. Хранится в
 * раскладке стола на шлюзе: одна на все устройства человека, своя у каждой роли. Голосом — «покажи ярлыки» и «спрячь
 * ярлыки», только пока переключатель на экране (фразы у кнопок, docs/plans/voice-creation.md, принцип 2).
 */
function DeskCard() {
    const panel = usePanel();
    const [layout, actions] = useDeskLayout(panel.role.role);
    useVoiceAction([tr.settings.shortcutsBarOnPhrase], () => actions.shortcutsBar(true));
    useVoiceAction([tr.settings.shortcutsBarOffPhrase], () => actions.shortcutsBar(false));
    return (
        <section class="card glass card-list" aria-labelledby="desk-h">
            <div class="section-head">
                <h2 id="desk-h" class="t-title-xs m-0">
                    {tr.settings.deskTitle}
                </h2>
            </div>
            <label class="w-toggle">
                <span class="flex-1 min-w-0 flex flex-col">
                    <span class="w-strong">{tr.settings.shortcutsBar}</span>
                    <span class="w-cap">{tr.settings.shortcutsBarAbout}</span>
                </span>
                <Switch on={!!layout.shortcutsBar} label={tr.settings.shortcutsBar} onChange={(on) => actions.shortcutsBar(on)} />
            </label>
        </section>
    );
}

/** «Пройти первые шаги»: экран первого запуска заново (screens/onboarding.tsx). */
function OnboardingCard() {
    return (
        <section class="card glass card-list" aria-labelledby="onb-again-h">
            <div class="section-head">
                <h2 id="onb-again-h" class="t-title-xs m-0">
                    {tr.onboarding.againTitle}
                </h2>
            </div>
            <p class="w-cap m-0">{tr.onboarding.againAbout}</p>
            <div>
                <button class="w-btn w-btn-ink" onClick={openOnboarding}>
                    {tr.onboarding.againButton}
                </button>
            </div>
        </section>
    );
}

/** Названия ролей на языке пульта: функция — словарь читается при отрисовке. */
const roleTitles = (): Record<Role, string> => ({ admin: tr.settings.roleAdmin, member: tr.settings.roleMember, guest: tr.settings.roleGuest });
const ROLES: Role[] = ['admin', 'member', 'guest'];

/** Буква-аватар человека или ключа. */
function Initial({ name }: { name: string }) {
    return <span class="win-row-ico win-initial">{name.trim().slice(0, 1).toUpperCase() || '·'}</span>;
}

/**
 * Люди дома и голос дома (холст «Окна DuoWave», 17.11): строки — имя, роль выбором, редкое за «⋯» (что нельзя голосом,
 * приостановить, удалить); добавить человека — своим окном справа.
 */
function People({ catalog }: { catalog: AccessCatalog | null }) {
    const [people, setPeople] = useState<Person[] | null>(null);
    const [household, setHousehold] = useState<Role | null>(null);
    const [error, setError] = useState<string | null>(null);
    const [busy, setBusy] = useState(false);
    const [open, setOpen] = useState<string | null>(null);
    const [adding, setAdding] = useState(false);
    const [name, setName] = useState('');
    const [password, setPassword] = useState('');
    const [role, setRole] = useState<Role>('member');

    const read = async () => {
        try {
            setPeople(await api.people());
            setHousehold((await api.household()).role);
            setError(null);
        } catch (e) {
            setError(problemOf(e));
        }
    };
    useEffect(() => void read(), []);

    const run = async (work: () => Promise<unknown>) => {
        setBusy(true);
        setError(null);
        try {
            await work();
            await read();
            return true;
        } catch (e) {
            setError(problemOf(e));
            return false;
        } finally {
            setBusy(false);
        }
    };

    const add = () =>
        void run(async () => {
            await api.addPerson({ name: name.trim(), role, ...(password ? { password } : {}) });
        }).then((ok) => {
            if (!ok) return;
            setName('');
            setPassword('');
            setAdding(false);
        });

    const toggleDeny = (person: Person, permission: string) => {
        const deny = new Set(person.restrictions.deny ?? []);
        if (deny.has(permission)) deny.delete(permission);
        else deny.add(permission);
        return run(() => api.updatePerson(person.id, { restrictions: { ...person.restrictions, deny: [...deny] } }));
    };
    const remove = async (person: Person) => {
        const yes = await ask({ title: tr.settings.removePersonTitle(person.name), text: tr.settings.removePersonText, ok: tr.settings.removeOk, danger: true });
        if (yes) void run(() => api.removePerson(person.id));
    };

    return (
        <section class="card glass card-list" aria-labelledby="people-h">
            <div class="section-head">
                <h2 id="people-h" class="t-title-xs m-0">
                    {tr.settings.people}
                </h2>
                {people && <span class="t-meta">{tr.settings.peopleCount(people.length)}</span>}
            </div>
            {error && (
                <p class="win-callout m-0 mb-8" role="alert">
                    {error}
                </p>
            )}
            {!people && !error ? (
                <SkeletonRows rows={2} icon />
            ) : (
                <div class="win-group">
                    {people?.map((person) => (
                        <div key={person.id} class="win-person">
                            <div class="win-row">
                                <Initial name={person.name} />
                                <div class="win-row-main">
                                    <span class="win-row-t">{person.name}</span>
                                    <span class="win-row-s">
                                        {person.canSignIn
                                            ? person.lastLoginAt
                                                ? tr.settings.signedInAgo(timeAgo(person.lastLoginAt)).replace(/^\s*·\s*/, '')
                                                : tr.settings.neverSignedIn.replace(/^\s*·\s*/, '')
                                            : tr.settings.noSignIn.replace(/^\s*·\s*/, '')}
                                        {person.restrictions.deny?.length ? tr.settings.denials(person.restrictions.deny.length) : ''}
                                    </span>
                                </div>
                                <select
                                    class="field"
                                    aria-label={tr.settings.roleOf(person.name)}
                                    value={person.role}
                                    disabled={busy}
                                    onChange={(e) => void run(() => api.updatePerson(person.id, { role: (e.target as HTMLSelectElement).value as Role }))}
                                >
                                    {ROLES.map((value) => (
                                        <option key={value} value={value}>
                                            {roleTitles()[value]}
                                        </option>
                                    ))}
                                </select>
                                <MoreMenu
                                    title={person.name}
                                    items={[
                                        { label: open === person.id ? tr.settings.collapse : tr.settings.voiceDenied, act: () => setOpen(open === person.id ? null : person.id) },
                                        {
                                            label: person.status === 'active' ? tr.settings.suspend : tr.settings.restore,
                                            disabled: busy,
                                            act: () => void run(() => api.updatePerson(person.id, { status: person.status === 'active' ? 'disabled' : 'active' })),
                                        },
                                        { label: tr.settings.removeAction, danger: true, disabled: busy, act: () => void remove(person) },
                                    ]}
                                />
                            </div>
                            {open === person.id && (
                                <div class="win-row is-col">
                                    <span class="win-row-s">{tr.settings.deniedByVoice}</span>
                                    <div class="win-tags">
                                        {(catalog?.capabilities ?? []).map((capability) => {
                                            const denied = person.restrictions.deny?.includes(capability.permission) ?? false;
                                            return (
                                                <button
                                                    key={capability.id}
                                                    class={`pill ${denied ? 'pill-pick' : 'pill-off'} cursor-pointer py-6 px-12 min-h-32`}
                                                    aria-pressed={denied}
                                                    disabled={busy}
                                                    onClick={() => void toggleDeny(person, capability.permission)}
                                                >
                                                    {denied ? '✕ ' : ''}
                                                    {capability.title}
                                                </button>
                                            );
                                        })}
                                    </div>
                                </div>
                            )}
                        </div>
                    ))}
                    <div class="win-row">
                        <div class="win-row-main">
                            <span class="win-row-t">{tr.settings.householdVoice}</span>
                            <span class="win-row-s">{byMode(tr.settings.householdVoiceAbout, tr.settings.householdVoiceAboutComputer)}</span>
                        </div>
                        <select
                            class="field"
                            aria-label={tr.settings.householdVoiceRole}
                            value={household ?? 'member'}
                            disabled={busy}
                            onChange={(e) => void run(() => api.setHousehold({ role: (e.target as HTMLSelectElement).value as Role }))}
                        >
                            <option value="member">{tr.settings.householdMember}</option>
                            <option value="guest">{tr.settings.householdGuest}</option>
                        </select>
                    </div>
                    <div class="win-row">
                        <button class="btn btn-plain btn-sm" onClick={() => setAdding(true)}>
                            <IconPlus size={14} />
                            {tr.settings.addPerson}
                        </button>
                    </div>
                </div>
            )}
            {adding && (
                <Sheet
                    title={tr.settings.newPerson}
                    onClose={() => setAdding(false)}
                    footer={
                        <>
                            <button class="btn btn-plain" onClick={() => setAdding(false)}>
                                {tr.ui.cancel}
                            </button>
                            <button class="btn btn-main" disabled={busy || name.trim().length < 2} aria-busy={busy || undefined} onClick={add}>
                                {tr.settings.addPerson}
                            </button>
                        </>
                    }
                >
                    <label class="flex flex-col gap-6">
                        <span class="win-label">{tr.settings.newPersonName}</span>
                        <input class="field" value={name} placeholder={tr.settings.namePlaceholder} data-autofocus onInput={(e) => setName((e.target as HTMLInputElement).value)} />
                    </label>
                    <label class="flex flex-col gap-6">
                        <span class="win-label">{tr.settings.passwordOptionalLabel}</span>
                        <input class="field" type="password" value={password} placeholder={tr.settings.passwordOptional} onInput={(e) => setPassword((e.target as HTMLInputElement).value)} />
                        <span class="win-hint">{byMode(tr.settings.noPasswordNote, tr.settings.noPasswordNoteComputer)}</span>
                    </label>
                    <label class="flex flex-col gap-6">
                        <span class="win-label">{tr.settings.newPersonRole}</span>
                        <select class="field" value={role} onChange={(e) => setRole((e.target as HTMLSelectElement).value as Role)}>
                            {ROLES.map((value) => (
                                <option key={value} value={value}>
                                    {roleTitles()[value]}
                                </option>
                            ))}
                        </select>
                    </label>
                    {error && (
                        <p class="win-callout m-0" role="alert">
                            {error}
                        </p>
                    )}
                </Sheet>
            )}
        </section>
    );
}

/** Группа права ключа: смотреть, делать, настраивать — по имени права. */
function keyGroup(id: string): 'see' | 'do' | 'manage' {
    if (id.endsWith('.view')) return 'see';
    if (['station.say', 'station.mode', 'mcp.use'].includes(id)) return 'do';
    return 'manage';
}

/**
 * Ключи для скриптов и агентов: ровно те права, что дал администратор; секрет показывается один раз. Строки — имя,
 * когда пользовались, права кодом; «Отозвать» — за «⋯»; новый ключ — своим окном с правами по смыслу.
 */
function Keys({ catalog }: { catalog: AccessCatalog | null }) {
    const [keys, setKeys] = useState<ApiKey[] | null>(null);
    const [creating, setCreating] = useState(false);
    const [busy, setBusy] = useState(false);
    const [name, setName] = useState('');
    const [grants, setGrants] = useState<string[]>(['station.view', 'station.say']);
    const [secret, setSecret] = useState<string | null>(null);
    const [error, setError] = useState<string | null>(null);
    const read = () =>
        api
            .apiKeys()
            .then(setKeys)
            .catch((e) => setError(problemOf(e)));
    useEffect(() => void read(), []);
    const create = async () => {
        setError(null);
        setBusy(true);
        try {
            const created = await api.createApiKey(name.trim(), grants);
            setSecret(created.secret);
            setName('');
            setCreating(false);
            await read();
        } catch (e) {
            setError(problemOf(e));
        } finally {
            setBusy(false);
        }
    };
    const revoke = async (key: ApiKey) => {
        const yes = await ask({ title: tr.settings.revokeTitle(key.name), text: tr.settings.revokeText, ok: tr.settings.revokeOk, danger: true });
        if (!yes) return;
        try {
            await api.revokeApiKey(key.id);
            await read();
        } catch (e) {
            setError(problemOf(e));
        }
    };
    const choices = (catalog?.permissions ?? []).filter((permission) => !['users.manage', 'audit.view'].includes(permission.id));
    const groups: Array<{ id: 'see' | 'do' | 'manage'; title: string }> = [
        { id: 'see', title: tr.settings.keyGroupSee },
        { id: 'do', title: tr.settings.keyGroupDo },
        { id: 'manage', title: tr.settings.keyGroupManage },
    ];
    const live = keys?.filter((key) => !key.revoked);
    return (
        <section class="card glass card-list" aria-labelledby="keys-h">
            <div class="section-head">
                <h2 id="keys-h" class="t-title-xs m-0">
                    {tr.settings.keys}
                </h2>
                <span class="t-meta">{tr.settings.keysFor}</span>
            </div>
            {error && !creating && (
                <p class="win-callout m-0 mb-8" role="alert">
                    {error}
                </p>
            )}
            {secret && (
                <div role="status" class="win-callout flex-col mb-8">
                    <b>{tr.settings.copyKeyNow}</b>
                    <code class="win-code break-all">{secret}</code>
                    <button class="btn btn-plain btn-sm self-start" onClick={() => setSecret(null)}>
                        {tr.settings.copiedKey}
                    </button>
                </div>
            )}
            {!keys && !error ? (
                <SkeletonRows rows={1} icon />
            ) : (
                <div class="win-group">
                    {live?.map((key) => (
                        <div key={key.id} class="win-row">
                            <span class="win-row-ico">
                                <IconKey size={16} />
                            </span>
                            <div class="win-row-main">
                                <span class="win-row-t">{key.name}</span>
                                <span class="win-row-s">{key.lastUsedAt ? tr.settings.usedAgo(timeAgo(key.lastUsedAt)) : tr.settings.neverUsed}</span>
                                <span class="win-tags mt-6">
                                    {key.grants.slice(0, 3).map((grant) => (
                                        <span key={grant} class="win-code">
                                            {grant}
                                        </span>
                                    ))}
                                    {key.grants.length > 3 && <span class="win-chip">{tr.settings.grantsMore(key.grants.length - 3)}</span>}
                                </span>
                            </div>
                            <MoreMenu title={key.name} items={[{ label: tr.settings.revokeAction, danger: true, act: () => void revoke(key) }]} />
                        </div>
                    ))}
                    <div class="win-row">
                        <button class="btn btn-plain btn-sm" onClick={() => (setError(null), setCreating(true))}>
                            <IconPlus size={14} />
                            {tr.settings.createKey}
                        </button>
                    </div>
                </div>
            )}
            {creating && (
                <Sheet
                    title={tr.settings.newKey}
                    meta={<span class="win-chip">{tr.settings.keysFor}</span>}
                    icon={
                        <span class="win-row-ico win-head-ico">
                            <IconKey size={20} />
                        </span>
                    }
                    onClose={() => setCreating(false)}
                    footer={
                        <>
                            <button class="btn btn-plain" onClick={() => setCreating(false)}>
                                {tr.ui.cancel}
                            </button>
                            <button class="btn btn-main" disabled={busy || name.trim().length < 2 || !grants.length} aria-busy={busy || undefined} onClick={() => void create()}>
                                {tr.settings.createKey}
                            </button>
                        </>
                    }
                >
                    <label class="flex flex-col gap-6">
                        <span class="win-label">{tr.settings.keyName}</span>
                        <input class="field" value={name} placeholder={tr.settings.keyPlaceholder} data-autofocus onInput={(e) => setName((e.target as HTMLInputElement).value)} />
                    </label>
                    <WinSection title={tr.settings.keyWhatCan} aside={tr.settings.keyChosen(grants.length, choices.length)}>
                        {groups.map((group) => {
                            const list = choices.filter((permission) => keyGroup(permission.id) === group.id);
                            if (list.length === 0) return null;
                            return (
                                <div key={group.id} class="flex flex-col gap-6">
                                    <span class="win-sec-title">{group.title}</span>
                                    <div class="win-group">
                                        {list.map((permission) => {
                                            const on = grants.includes(permission.id);
                                            return (
                                                <label key={permission.id} class="win-row win-check">
                                                    <input
                                                        type="checkbox"
                                                        checked={on}
                                                        onChange={() => setGrants(on ? grants.filter((grant) => grant !== permission.id) : [...grants, permission.id])}
                                                    />
                                                    <span class="win-row-main win-row-t">{permission.title}</span>
                                                    <span class="win-code">{permission.id}</span>
                                                </label>
                                            );
                                        })}
                                    </div>
                                </div>
                            );
                        })}
                    </WinSection>
                    {error && (
                        <p class="win-callout m-0" role="alert">
                            {error}
                        </p>
                    )}
                </Sheet>
            )}
        </section>
    );
}

/** Подключение в «Истории изменений»: какое (поля и значения шлюз в историю не пишет). */
const connectorTitles = (): Record<string, string> => ({
    'home-assistant': 'Home Assistant',
    telegram: 'Telegram',
    livekit: 'LiveKit',
    sip: tr.settings.connectorTelephony,
    weather: tr.settings.connectorWeather,
    news: tr.settings.connectorNews,
});


/** Кто что менял — по кнопке: список длинный, а нужен редко. */
function History() {
    const [open, setOpen] = useState(false);
    // Окно — рядом с карточкой, не внутри: стекло карточки (backdrop-filter) заперло бы его в своих границах.
    return (
        <>
            <section class="card glass card-list" aria-labelledby="history-h">
                <div class="section-head">
                    <h2 id="history-h" class="t-title-xs m-0">
                        {tr.settings.history}
                    </h2>
                    <button class="pill pill-off pill-btn" onClick={() => setOpen(true)}>
                        {tr.settings.openAction}
                    </button>
                </div>
                <p class="t-caption m-0">
                    {byMode(tr.settings.historyAbout, tr.settings.historyAboutComputer)}
                </p>
            </section>
            {open && <HistorySheet onClose={() => setOpen(false)} />}
        </>
    );
}

/** Последние сто изменений, новые сверху. */
function HistorySheet({ onClose }: { onClose: () => void }) {
    const [entries, setEntries] = useState<AuditEntry[] | null>(null);
    const [error, setError] = useState<string | null>(null);
    useEffect(() => {
        void api
            .audit(100)
            .then(setEntries)
            .catch((e) => setError(problemOf(e)));
    }, []);
    return (
        <Sheet title={tr.settings.history} about={tr.settings.historySheetAbout} icon={<span class="route-icon w-38 h-38 flex-none" aria-hidden="true"><IconJournal size={19} /></span>} onClose={onClose}>
            {error && (
                <p class="t-caption c-dot-offline m-0" role="alert">
                    {error}
                </p>
            )}
            {!entries && !error && (
                <p class="t-meta m-0">
                    {tr.settings.askingGateway}
                </p>
            )}
            {entries?.length === 0 && (
                <p class="t-meta m-0">
                    {tr.settings.nothingChanged}
                </p>
            )}
            {!!entries?.length && (
                <div>
                    {entries.map((entry) => (
                        <div
                            key={entry.id}
                            class="flex justify-between gap-12 py-8 px-0 bt-divider"
                        >
                            <span>
                                <b>{tr.settings.actions[entry.action] ?? entry.action}</b>
                                {entry.action === 'connector.changed' && typeof entry.details.connector === 'string'
                                    ? ` ${connectorTitles()[entry.details.connector] ?? entry.details.connector}`
                                    : ''}
                                {entry.actor?.name ? ` · ${entry.actor.name}` : ''}
                            </span>
                            <span class="t-caption flex-none">
                                {timeAgo(entry.at)}
                            </span>
                        </div>
                    ))}
                </div>
            )}
        </Sheet>
    );
}

/** Свой вход: пароль, выход отсюда и отовсюду. */
function Account({ onSignOut }: { onSignOut: () => void }) {
    const [current, setCurrent] = useState('');
    const [next, setNext] = useState('');
    const [message, setMessage] = useState<{ ok: boolean; text: string } | null>(null);
    const session = sessionStore.get();
    const change = async () => {
        try {
            const { token } = await api.changePassword(current, next);
            if (session) sessionStore.set({ ...session, token });
            setCurrent('');
            setNext('');
            setMessage({ ok: true, text: tr.settings.passwordChanged });
        } catch (e) {
            setMessage({ ok: false, text: problemOf(e) });
        }
    };
    const everywhere = async () => {
        if (!(await ask({ title: tr.settings.signOutEverywhereTitle, text: tr.settings.signOutEverywhereText, ok: tr.settings.signOutEverywhereOk, danger: true }))) return;
        try {
            await api.logoutEverywhere();
        } finally {
            onSignOut();
        }
    };
    return (
        <section class="card glass card-list" aria-labelledby="account-h">
            <div class="section-head">
                <h2 id="account-h" class="t-title-xs m-0">
                    {tr.settings.myAccount}
                </h2>
                <span class="t-meta">{session?.name}</span>
            </div>
            <div class="flex gap-8 flex-wrap">
                <input
                    class="field flex-1 min-w-120"
                    type="password"
                    value={current}
                    placeholder={tr.settings.currentPassword}
                    aria-label={tr.settings.currentPassword}
                    onInput={(e) => setCurrent((e.target as HTMLInputElement).value)}
                />
                <input
                    class="field flex-1 min-w-120"
                    type="password"
                    value={next}
                    placeholder={tr.settings.newPasswordPlaceholder}
                    aria-label={tr.settings.newPassword}
                    onInput={(e) => setNext((e.target as HTMLInputElement).value)}
                />
                <button
                    class="pill pill-own pill-btn"
                    disabled={!current || next.length < 8}
                    onClick={() => void change()}
                >
                    {tr.settings.changeAction}
                </button>
            </div>
            {message && (
                <p
                    class={`t-caption m-0 mt-8 ${message.ok ? 'c-accent-text' : 'c-dot-offline'}`}
                    role={message.ok ? 'status' : 'alert'}
                >
                    {message.text}
                </p>
            )}
            <div class="flex gap-8 flex-wrap mt-10">
                <button class="pill pill-off pill-btn" onClick={onSignOut}>
                    {tr.settings.signOut}
                </button>
                <button class="pill pill-off pill-btn" onClick={() => void everywhere()}>
                    {tr.settings.signOutEverywhere}
                </button>
            </div>
        </section>
    );
}
