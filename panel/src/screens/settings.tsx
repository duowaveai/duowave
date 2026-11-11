import { useEffect, useState } from 'preact/hooks';

import { AccessCatalog, ApiKey, AuditEntry, Person, Role, api, can, plural, problemOf, sessionStore } from '../api';
import { IconJournal, IconKey, IconPalette } from '../icons';
import { timeAgo } from '../time';
import { ThemeCard } from '../shell/theme-picker';
import { Sheet } from '../ui/sheet';
import { SectionTabs, TabEntry } from '../ui/tabs';
import { ask } from '../ui/confirm';

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
        { id: 'access', label: 'Доступ', icon: <IconKey size={16} /> },
        { id: 'look', label: 'Оформление', icon: <IconPalette size={16} /> },
    ];
    const shown = tabs.some((entry) => entry.id === tab) ? tab : tabs[0].id;
    return (
        <>
            {!bare && <SectionTabs label="Настройки" tabs={tabs} tab={shown} onPick={onTab} />}
            {shown === 'access' && (
                <>
                    {can('users.manage') && <People catalog={catalog} />}
                    {can('users.manage') && <Keys catalog={catalog} />}
                    <Account onSignOut={onSignOut} />
                    {can('audit.view') && <History />}
                </>
            )}
            {shown === 'look' && <ThemeCard />}
        </>
    );
}

const ROLE_TITLES: Record<Role, string> = { admin: 'Администратор', member: 'Домашний', guest: 'Гость' };

/** Люди дома и голос дома: роль, ограничения на возможности, вход в пульт. */
function People({ catalog }: { catalog: AccessCatalog | null }) {
    const [people, setPeople] = useState<Person[] | null>(null);
    const [household, setHousehold] = useState<Role | null>(null);
    const [error, setError] = useState<string | null>(null);
    const [busy, setBusy] = useState(false);
    const [open, setOpen] = useState<string | null>(null);
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
        } catch (e) {
            setError(problemOf(e));
        } finally {
            setBusy(false);
        }
    };

    const add = () =>
        run(async () => {
            await api.addPerson({ name: name.trim(), role, ...(password ? { password } : {}) });
            setName('');
            setPassword('');
        });

    const toggleDeny = (person: Person, permission: string) => {
        const deny = new Set(person.restrictions.deny ?? []);
        if (deny.has(permission)) deny.delete(permission);
        else deny.add(permission);
        return run(() => api.updatePerson(person.id, { restrictions: { ...person.restrictions, deny: [...deny] } }));
    };

    return (
        <section class="card glass card-list" aria-labelledby="people-h">
            <div class="section-head">
                <h2 id="people-h" class="t-title-xs m-0">
                    Люди
                </h2>
                {people && (
                    <span class="t-meta">
                        {people.length} {plural(people.length, 'человек', 'человека', 'человек')}
                    </span>
                )}
            </div>
            {error && (
                <p class="t-caption c-dot-offline m-0 mb-8" role="alert">
                    {error}
                </p>
            )}

            <div class="list-row">
                <div class="flex-1 min-w-160 flex flex-col gap-2">
                    <span class="fw-700">Голос дома</span>
                    <span class="t-caption">Кто говорит с колонкой, пока она не узнаёт людей по голосу или по имени.</span>
                </div>
                <select
                    class="field"
                    aria-label="Роль голоса дома"
                    value={household ?? 'member'}
                    disabled={busy}
                    onChange={(e) => void run(() => api.setHousehold({ role: (e.target as HTMLSelectElement).value as Role }))}
                >
                    <option value="member">Домашний — всё, кроме настроек</option>
                    <option value="guest">Гость — простые вещи</option>
                </select>
            </div>

            {people?.map((person) => (
                <div
                    key={person.id}
                    class="flex flex-col gap-8 py-10 px-0 bt-divider"
                >
                    <div class="flex items-center gap-12 flex-wrap">
                        <div class="flex-1 min-w-160 flex flex-col gap-2">
                            <span class="fw-700">{person.name}</span>
                            <span class="t-caption">
                                {ROLE_TITLES[person.role]}
                                {person.canSignIn
                                    ? person.lastLoginAt
                                        ? ` · входил ${timeAgo(person.lastLoginAt)}`
                                        : ' · ещё не входил'
                                    : ' · без входа в пульт'}
                                {person.restrictions.deny?.length ? ` · запретов: ${person.restrictions.deny.length}` : ''}
                            </span>
                        </div>
                        <select
                            class="field"
                            aria-label={`Роль: ${person.name}`}
                            value={person.role}
                            disabled={busy}
                            onChange={(e) =>
                                void run(() => api.updatePerson(person.id, { role: (e.target as HTMLSelectElement).value as Role }))
                            }
                        >
                            {(Object.keys(ROLE_TITLES) as Role[]).map((value) => (
                                <option key={value} value={value}>
                                    {ROLE_TITLES[value]}
                                </option>
                            ))}
                        </select>
                        <button
                            class="pill pill-off pill-btn"
                            aria-expanded={open === person.id}
                            onClick={() => setOpen(open === person.id ? null : person.id)}
                        >
                            {open === person.id ? 'свернуть' : 'права'}
                        </button>
                    </div>
                    {open === person.id && (
                        <div class="flex flex-col gap-8">
                            <span class="t-caption">Нельзя голосом — снимите, чтобы разрешить снова:</span>
                            <div class="flex flex-wrap gap-6">
                                {(catalog?.capabilities ?? []).map((capability) => {
                                    const denied = person.restrictions.deny?.includes(capability.permission) ?? false;
                                    return (
                                        <button
                                            key={capability.id}
                                            class={`pill ${denied ? 'pill-yandex' : 'pill-off'} cursor-pointer py-6 px-12 min-h-36`}
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
                            <div class="flex gap-8 flex-wrap">
                                <button
                                    class="pill pill-off pill-btn"
                                    disabled={busy}
                                    onClick={() =>
                                        void run(() =>
                                            api.updatePerson(person.id, { status: person.status === 'active' ? 'disabled' : 'active' }),
                                        )
                                    }
                                >
                                    {person.status === 'active' ? 'приостановить' : 'вернуть'}
                                </button>
                                <button
                                    class="pill pill-off pill-btn"
                                    disabled={busy}
                                    onClick={async () => {
                                        const yes = await ask({
                                            title: `Удалить «${person.name}»?`,
                                            text: 'Его настройки уйдут вместе с ним.',
                                            ok: 'Удалить',
                                            danger: true,
                                        });
                                        if (yes) void run(() => api.removePerson(person.id));
                                    }}
                                >
                                    удалить
                                </button>
                            </div>
                        </div>
                    )}
                </div>
            ))}

            <div class="flex gap-8 flex-wrap pt-10 bt-divider">
                <input
                    class="field flex-1 min-w-120"
                    value={name}
                    placeholder="Имя"
                    aria-label="Имя нового человека"
                    onInput={(e) => setName((e.target as HTMLInputElement).value)}
                />
                <input
                    class="field flex-1 min-w-120"
                    type="password"
                    value={password}
                    placeholder="Пароль (не обязательно)"
                    aria-label="Пароль для входа в пульт — не обязательно"
                    onInput={(e) => setPassword((e.target as HTMLInputElement).value)}
                />
                <select
                    class="field"
                    aria-label="Роль нового человека"
                    value={role}
                    onChange={(e) => setRole((e.target as HTMLSelectElement).value as Role)}
                >
                    {(Object.keys(ROLE_TITLES) as Role[]).map((value) => (
                        <option key={value} value={value}>
                            {ROLE_TITLES[value]}
                        </option>
                    ))}
                </select>
                <button
                    class="pill pill-own pill-btn"
                    disabled={busy || name.trim().length < 2}
                    onClick={() => void add()}
                >
                    добавить
                </button>
            </div>
            <p class="t-caption m-0 mt-8">
                Без пароля человек не входит в пульт — это профиль, который колонка будет узнавать.
            </p>
        </section>
    );
}

/** Ключи для скриптов и агентов: ровно те права, что дал администратор; секрет показывается один раз. */
function Keys({ catalog }: { catalog: AccessCatalog | null }) {
    const [keys, setKeys] = useState<ApiKey[] | null>(null);
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
        try {
            const created = await api.createApiKey(name.trim(), grants);
            setSecret(created.secret);
            setName('');
            await read();
        } catch (e) {
            setError(problemOf(e));
        }
    };
    const revoke = async (key: ApiKey) => {
        const yes = await ask({ title: `Отозвать ключ «${key.name}»?`, text: 'Всё, что им пользуется, перестанет работать.', ok: 'Отозвать', danger: true });
        if (!yes) return;
        try {
            await api.revokeApiKey(key.id);
            await read();
        } catch (e) {
            setError(problemOf(e));
        }
    };
    const choices = (catalog?.permissions ?? []).filter((permission) => !['users.manage', 'audit.view'].includes(permission.id));
    return (
        <section class="card glass card-list" aria-labelledby="keys-h">
            <div class="section-head">
                <h2 id="keys-h" class="t-title-xs m-0">
                    Ключи доступа
                </h2>
                <span class="t-meta">для скриптов и агентов</span>
            </div>
            {error && (
                <p class="t-caption c-dot-offline m-0 mb-8" role="alert">
                    {error}
                </p>
            )}
            {secret && (
                <div
                    role="status"
                    class="py-12 px-14 r-panel bg-pill-yandex-bg c-pill-yandex-fg mb-8"
                >
                    <p class="m-0 mb-6 fs-13 fw-700">Скопируйте ключ сейчас — больше он не покажется:</p>
                    <code class="break-all fs-13">{secret}</code>
                    <div class="mt-8">
                        <button class="pill pill-off pill-btn-sm" onClick={() => setSecret(null)}>
                            скопировал
                        </button>
                    </div>
                </div>
            )}
            {keys
                ?.filter((key) => !key.revoked)
                .map((key) => (
                    <div
                        key={key.id}
                        class="list-row"
                    >
                        <div class="flex-1 min-w-160 flex flex-col gap-2">
                            <span class="fw-700">{key.name}</span>
                            <span class="t-caption">
                                {key.grants.join(', ')} ·{' '}
                                {key.lastUsedAt ? `пользовались ${timeAgo(key.lastUsedAt)}` : 'ещё не пользовались'}
                            </span>
                        </div>
                        <button class="pill pill-off pill-btn" onClick={() => void revoke(key)}>
                            отозвать
                        </button>
                    </div>
                ))}
            <div class="flex flex-col gap-8 pt-10 bt-divider">
                <input
                    class="field"
                    value={name}
                    placeholder="Для чего ключ: OpenClaw, скрипт…"
                    aria-label="Имя ключа"
                    onInput={(e) => setName((e.target as HTMLInputElement).value)}
                />
                <div class="flex flex-wrap gap-6">
                    {choices.map((permission) => {
                        const on = grants.includes(permission.id);
                        return (
                            <button
                                key={permission.id}
                                class={`pill ${on ? 'pill-own' : 'pill-off'} cursor-pointer py-6 px-12 min-h-36`}
                                aria-pressed={on}
                                title={permission.title}
                                onClick={() =>
                                    setGrants(on ? grants.filter((grant) => grant !== permission.id) : [...grants, permission.id])
                                }
                            >
                                {permission.id}
                            </button>
                        );
                    })}
                </div>
                <button
                    class="pill pill-own cursor-pointer py-8 px-14 self-start"
                    disabled={name.trim().length < 2 || !grants.length}
                    onClick={() => void create()}
                >
                    создать ключ
                </button>
            </div>
        </section>
    );
}

/** Подключение в «Истории изменений»: какое (поля и значения шлюз в историю не пишет). */
const CONNECTOR_TITLES: Record<string, string> = {
    'home-assistant': 'Home Assistant',
    telegram: 'Telegram',
    livekit: 'LiveKit',
    sip: 'телефония',
    weather: 'погода',
    news: 'новости',
};

const ACTIONS: Record<string, string> = {
    'mode.changed': 'режим',
    'routes.changed': 'маршруты',
    'voice.changed': 'голос',
    'persona.changed': 'характер',
    'settings.changed': 'настройка',
    'model.installed': 'скачана модель',
    'model.removed': 'удалена модель',
    'model.server.restarted': 'перезапуск сервера',
    'connector.changed': 'подключение',
    'brain.changed': 'модели',
    'mcp.changed': 'MCP-сервер',
    'user.created': 'новый человек',
    'user.updated': 'изменён человек',
    'user.deleted': 'удалён человек',
    'apiKey.created': 'новый ключ',
    'apiKey.revoked': 'отозван ключ',
    'auth.login': 'вход',
    'auth.failed': 'неверный пароль',
    'station.pending': 'новая колонка просится',
    'station.updated': 'колонка',
    'station.switched': 'голос колонки',
    'station.firmware': 'прошивка колонки обновилась',
    'safety.fired': 'страховка колонки',
};

/** Кто что менял — по кнопке: список длинный, а нужен редко. */
function History() {
    const [open, setOpen] = useState(false);
    // Окно — рядом с карточкой, не внутри: стекло карточки (backdrop-filter) заперло бы его в своих границах.
    return (
        <>
            <section class="card glass card-list" aria-labelledby="history-h">
                <div class="section-head">
                    <h2 id="history-h" class="t-title-xs m-0">
                        История изменений
                    </h2>
                    <button class="pill pill-off pill-btn" onClick={() => setOpen(true)}>
                        открыть
                    </button>
                </div>
                <p class="t-caption m-0">
                    Кто что менял: настройки, люди, колонки и входы.
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
        <Sheet title="История изменений" about="Кто что менял, новые сверху" icon={<span class="route-icon w-38 h-38 flex-none" aria-hidden="true"><IconJournal size={19} /></span>} onClose={onClose}>
            {error && (
                <p class="t-caption c-dot-offline m-0" role="alert">
                    {error}
                </p>
            )}
            {!entries && !error && (
                <p class="t-meta m-0">
                    Спрашиваем шлюз…
                </p>
            )}
            {entries?.length === 0 && (
                <p class="t-meta m-0">
                    Пока ничего не меняли.
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
                                <b>{ACTIONS[entry.action] ?? entry.action}</b>
                                {entry.action === 'connector.changed' && typeof entry.details.connector === 'string'
                                    ? ` ${CONNECTOR_TITLES[entry.details.connector] ?? entry.details.connector}`
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
            setMessage({ ok: true, text: 'Пароль сменён: на других устройствах войдите заново.' });
        } catch (e) {
            setMessage({ ok: false, text: problemOf(e) });
        }
    };
    const everywhere = async () => {
        if (!(await ask({ title: 'Выйти на всех устройствах?', text: 'И на этом тоже.', ok: 'Выйти везде', danger: true }))) return;
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
                    Мой вход
                </h2>
                <span class="t-meta">{session?.name}</span>
            </div>
            <div class="flex gap-8 flex-wrap">
                <input
                    class="field flex-1 min-w-120"
                    type="password"
                    value={current}
                    placeholder="Текущий пароль"
                    aria-label="Текущий пароль"
                    onInput={(e) => setCurrent((e.target as HTMLInputElement).value)}
                />
                <input
                    class="field flex-1 min-w-120"
                    type="password"
                    value={next}
                    placeholder="Новый, от 8 символов"
                    aria-label="Новый пароль"
                    onInput={(e) => setNext((e.target as HTMLInputElement).value)}
                />
                <button
                    class="pill pill-own pill-btn"
                    disabled={!current || next.length < 8}
                    onClick={() => void change()}
                >
                    сменить
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
                    выйти
                </button>
                <button class="pill pill-off pill-btn" onClick={() => void everywhere()}>
                    выйти везде
                </button>
            </div>
        </section>
    );
}
