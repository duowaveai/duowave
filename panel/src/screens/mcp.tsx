import { useEffect, useState } from 'preact/hooks';

import { McpPreset, McpServer, McpServerInput, McpState, McpTool, McpView, api, can, problemOf } from '../api';
import { BRANDS, type BrandId } from '../brand-icons';
import { CAPABILITIES } from '../capabilities';
import { IconBraces, IconGlobe, IconPage, IconTerminal } from '../icons';
import { BrandMark } from '../ui/brand';
import { Action } from '../ui/cards';
import { Sheet } from '../ui/sheet';
import { Switch } from '../ui/switch';
import { ask } from '../ui/confirm';

const STATE: Record<McpState, { text: string; cls: string }> = {
    ready: { text: 'работает', cls: 'pill-own' },
    connecting: { text: 'подключается', cls: 'pill-off' },
    login: { text: 'нужно войти', cls: 'pill-yandex' },
    error: { text: 'не отвечает', cls: 'pill-error' },
    off: { text: 'выключен', cls: 'pill-off' },
};

/** Значок сервера: знак сервиса, иначе — программа на Mac или адрес. */
function ServerIcon({ brand, transport, preset, size = 38 }: { brand?: string; transport: 'stdio' | 'http'; preset?: string | null; size?: number }) {
    const glyph = Math.round(size / 2);
    return (
        <span class="route-icon" aria-hidden="true" style={{ width: size, height: size, flex: 'none' }}>
            {brand && brand in BRANDS ? (
                <BrandMark id={brand as BrandId} size={glyph} />
            ) : preset === 'fetch' ? (
                <IconPage size={glyph} />
            ) : transport === 'stdio' ? (
                <IconTerminal size={glyph} />
            ) : (
                <IconGlobe size={glyph} />
            )}
        </span>
    );
}

/** «NAME=value» по строке → переменные; пустые строки и строки без «=» пропускаются. */
function pairs(text: string, separator: '=' | ':'): Record<string, string> {
    const out: Record<string, string> = {};
    for (const line of text.split('\n')) {
        const at = line.indexOf(separator);
        if (at <= 0) continue;
        out[line.slice(0, at).trim()] = line.slice(at + 1).trim();
    }
    return out;
}

const hostOf = (url?: string): string => {
    try {
        return new URL(url ?? '').host;
    } catch {
        return url ?? '';
    }
};

/** Где сервер: команда программы или хост адреса. */
const whereOf = (server: McpServer): string =>
    server.transport === 'stdio' ? [server.command, ...(server.args ?? [])].join(' ') : hostOf(server.url);

/**
 * «MCP-серверы» в «Дом» → «Коннекторы»: подключённые серверы с состоянием и числом включённых инструментов, «Добавить» —
 * готовые, свои и вставка JSON, лист сервера — инструменты, инструкция, подсказка, права.
 */
export function McpSection({ onAsk }: { onAsk: () => void }) {
    const [view, setView] = useState<McpView | null>(null);
    const [error, setError] = useState<string | null>(null);
    const [adding, setAdding] = useState(false);
    const [open, setOpen] = useState<string | null>(null);
    const mayManage = can('connectors.manage');

    const load = () =>
        api
            .mcp()
            .then((fresh) => {
                setView(fresh);
                setError(null);
            })
            .catch((e) => setError(problemOf(e)));

    useEffect(() => {
        void load();
        const id = setInterval(() => void load(), 5000);
        return () => clearInterval(id);
    }, []);

    const opened = view?.servers.find((server) => server.id === open);
    // Окна — вне стеклянной карточки: её backdrop-filter стал бы опорой их position: fixed.
    return (
        <>
        <section class="card glass card-list" aria-labelledby="mcp-h">
            <div class="flex items-center justify-between min-h-44 gap-8 flex-wrap">
                <h2 id="mcp-h" class="t-title-xs m-0">
                    MCP-серверы
                </h2>
                <div class="flex gap-6 flex-wrap">
                    {can('mcp.use') && <Action label="спросить мозг" onClick={onAsk} />}
                    {mayManage && <Action label="добавить" tone="on" onClick={() => setAdding(true)} />}
                </div>
            </div>
            {error && (
                <p class="t-caption c-dot-offline m-0 mb-8" role="alert">
                    {error}
                </p>
            )}
            {view && view.toolsOn > view.toolsLimit && (
                <p class="t-caption c-pill-yandex-fg m-0 mb-8" role="status">
                    Включено инструментов: {view.toolsOn}. Больше {view.toolsLimit} — модель читает их на каждый вопрос и отвечает
                    медленнее; выключите лишние.
                </p>
            )}
            {view && !view.servers.length && (
                <p class="t-caption m-0 mb-8">
                    Серверы дают модели инструменты: управлять домом, искать в интернете, вести задачи и заметки. Добавьте готовый
                    или свой.
                </p>
            )}
            {view?.servers.map((server) => {
                const on = server.tools.filter((tool) => tool.on).length;
                const changed = server.tools.filter((tool) => tool.changed).length;
                return (
                    <button
                        key={server.id}
                        class="dash-option flex items-center gap-12 w-full text-left py-10 px-0 bt-divider cursor-pointer"
                        onClick={() => setOpen(server.id)}
                    >
                        <ServerIcon brand={server.brand} transport={server.transport} preset={server.preset} />
                        <span class="flex-1 min-w-0 flex flex-col gap-2">
                            <b class="flex gap-8 items-center flex-wrap">
                                {server.title}
                                <span class={`pill ${server.cloud ? 'pill-yandex' : 'pill-own'}`}>{server.cloud ? 'через интернет' : 'дома'}</span>
                            </b>
                            <span class="t-caption">
                                {server.state === 'ready'
                                    ? `инструментов включено: ${on} из ${server.tools.length}${changed ? ` · изменились: ${changed}` : ''}${
                                          server.instructions.text && !server.instructions.approved ? ' · инструкция ждёт одобрения' : ''
                                      }`
                                    : (server.detail ?? whereOf(server))}
                            </span>
                        </span>
                        <span class={`pill ${STATE[server.state].cls}`}>{STATE[server.state].text}</span>
                    </button>
                );
            })}
        </section>
            {adding && view && (
                <AddServer
                    presets={view.presets}
                    onClose={() => setAdding(false)}
                    onAdded={(server) => {
                        setAdding(false);
                        void load();
                        if (server) setOpen(server.id);
                    }}
                />
            )}
            {opened && (
                <ServerSheet
                    server={opened}
                    onClose={() => setOpen(null)}
                    onChanged={(fresh) => (fresh ? setView({ ...view!, servers: view!.servers.map((s) => (s.id === fresh.id ? fresh : s)) }) : void load())}
                />
            )}
        </>
    );
}

type Mode = { kind: 'pick' } | { kind: 'preset'; preset: McpPreset } | { kind: 'program' } | { kind: 'address' } | { kind: 'json' };

/** Шит «Добавить сервер»: готовые (со знаками), своя программа, свой адрес, вставка JSON. */
function AddServer({ presets, onClose, onAdded }: { presets: McpPreset[]; onClose: () => void; onAdded: (server?: McpServer) => void }) {
    const [mode, setMode] = useState<Mode>({ kind: 'pick' });
    const [values, setValues] = useState<Record<string, string>>({});
    const [busy, setBusy] = useState(false);
    const [error, setError] = useState<string | null>(null);
    const [imported, setImported] = useState<{ added: McpServer[]; problems: string[] } | null>(null);
    const set = (name: string) => (e: Event) => setValues((all) => ({ ...all, [name]: (e.target as HTMLInputElement).value }));
    const value = (name: string) => values[name] ?? '';

    const submit = async (input: McpServerInput) => {
        setBusy(true);
        setError(null);
        try {
            onAdded(await api.addMcpServer(input));
        } catch (e) {
            setError(problemOf(e));
        } finally {
            setBusy(false);
        }
    };

    const field = (name: string, title: string, options: { secret?: boolean; placeholder?: string; area?: boolean } = {}) => (
        <label key={name} class="flex flex-col gap-4">
            <span class="t-caption">{title}</span>
            {options.area ? (
                <textarea
                    class="field r-16 min-h-72 resize-y"
                    placeholder={options.placeholder}
                    value={value(name)}
                    onInput={set(name)}
                />
            ) : (
                <input
                    class="field"
                    type={options.secret ? 'password' : 'text'}
                    autocomplete="off"
                    placeholder={options.placeholder}
                    value={value(name)}
                    onInput={set(name)}
                />
            )}
        </label>
    );

    const footer = (label: string, ready: boolean, go: () => void) => (
        <div class="flex gap-8 flex-wrap">
            <button class="pill pill-own dash-confirm-btn" disabled={busy || !ready} onClick={go}>
                {busy ? 'Подключаем…' : label}
            </button>
            <button class="pill pill-off dash-confirm-btn" onClick={() => (setMode({ kind: 'pick' }), setError(null), setImported(null))}>
                Назад
            </button>
        </div>
    );

    if (mode.kind === 'pick') {
        const tile = (key: string, icon: preact.ComponentChildren, title: string, text: string, pick: () => void, disabled = false) => (
            <button
                key={key}
                class={`dash-option flex items-center gap-12 text-left border-glass r-16 py-10 px-12 ${disabled ? 'cursor-default opacity-50' : 'cursor-pointer'}`}
                disabled={disabled}
                onClick={pick}
            >
                {icon}
                <span class="flex flex-col gap-2 min-w-0">
                    <b>{title}</b>
                    <span class="t-caption">{text}</span>
                </span>
            </button>
        );
        return (
            <Sheet title="Добавить MCP-сервер" about="Готовые — с проверенными настройками; свои — на ваше усмотрение" onClose={onClose}>
                <div class="grid grid-fill-220 gap-8">
                    {presets.map((preset) =>
                        tile(
                            preset.id,
                            <ServerIcon brand={preset.brand} transport={preset.transport} preset={preset.id} />,
                            preset.title,
                            preset.ready ? preset.description : (preset.why ?? preset.description),
                            () => (setValues({ note: preset.note ?? '' }), setMode({ kind: 'preset', preset })),
                        ),
                    )}
                    {tile('program', <ServerIcon transport="stdio" />, 'Своя программа', 'Команда на этом Mac: npx, uvx, свой файл.', () => setMode({ kind: 'program' }))}
                    {tile('address', <ServerIcon transport="http" />, 'Свой адрес', 'Сервер по адресу: Streamable HTTP или старый SSE.', () => setMode({ kind: 'address' }))}
                    {tile(
                        'json',
                        <span class="route-icon w-38 h-38 flex-none" aria-hidden="true">
                            <IconBraces size={19} />
                        </span>,
                        'Вставить JSON',
                        'Как у Claude Desktop, Cursor или VS Code: mcpServers.',
                        () => setMode({ kind: 'json' }),
                    )}
                </div>
            </Sheet>
        );
    }

    if (mode.kind === 'preset') {
        const { preset } = mode;
        const needsUrl = !preset.ready;
        const missing = preset.fields.some((entry) => entry.secret && !value(entry.name).trim()) || (needsUrl && !value('url').trim());
        return (
            <Sheet
                title={preset.title}
                about={preset.transport === 'stdio' ? 'программа на этом Mac' : preset.outside ? 'сервер в интернете' : 'сервер по адресу'}
                icon={<ServerIcon brand={preset.brand} transport={preset.transport} preset={preset.id} />}
                onClose={onClose}
                footer={footer(preset.auth === 'oauth' ? 'Подключить' : 'Подключить', !missing, () =>
                    void submit({
                        preset: preset.id,
                        ...(needsUrl ? { url: value('url').trim() } : {}),
                        env: Object.fromEntries(preset.fields.filter((entry) => entry.name !== 'token').map((entry) => [entry.name, value(entry.name).trim()])),
                        ...(preset.fields.some((entry) => entry.name === 'token') ? { token: value('token').trim() } : {}),
                        note: value('note'),
                    }),
                )}
            >
                <p class="t-caption m-0">
                    {preset.description}
                </p>
                {needsUrl && field('url', 'Адрес Home Assistant', { placeholder: 'http://homeassistant.local:8123' })}
                {preset.fields.map((entry) => (
                    <div key={entry.name} class="flex flex-col gap-4">
                        {field(entry.name, entry.title, { secret: entry.secret, placeholder: entry.placeholder })}
                        {entry.url && (
                            <a class="t-caption c-accent-text" href={entry.url} target="_blank" rel="noopener noreferrer">
                                Где взять
                            </a>
                        )}
                    </div>
                ))}
                {preset.id === 'homeassistant' && (
                    <p class="t-caption m-0">
                        В Home Assistant один раз: «Настройки → Устройства и службы → Добавить → Model Context Protocol Server», в её
                        настройках включите вход долгоживущим токеном. Модели будут видны только устройства, открытые голосовым
                        помощникам.
                    </p>
                )}
                {preset.auth === 'oauth' && (
                    <p class="t-caption m-0">
                        После «Подключить» нажмите «Войти» в листе сервера: откроется страница сервиса, вход делаете вы сами.
                    </p>
                )}
                {preset.install && (
                    <p class="t-caption m-0">
                        Первый запуск скачает пакет {preset.install.package} через {preset.install.runner} — несколько мегабайт, один раз.
                    </p>
                )}
                {field('note', 'Подсказка модели: зачем этот сервер в доме', { area: true })}
                <p class="t-caption m-0">
                    Ключи хранятся в шлюзе зашифрованными. Инструменты, которые меняют данные, включаете вы сами.
                </p>
                {error && (
                    <p role="alert" class="t-meta m-0 c-dot-offline">
                        {error}
                    </p>
                )}
            </Sheet>
        );
    }

    if (mode.kind === 'program') {
        return (
            <Sheet
                title="Своя программа"
                about="MCP-сервер, который шлюз запустит на этом Mac"
                icon={<ServerIcon transport="stdio" />}
                onClose={onClose}
                footer={footer('Подключить', !!value('command').trim(), () =>
                    void submit({
                        transport: 'stdio',
                        title: value('title').trim() || undefined,
                        command: value('command').trim(),
                        args: value('args').trim(),
                        env: pairs(value('env'), '='),
                    }),
                )}
            >
                {field('title', 'Название', { placeholder: 'Например, Файлы' })}
                {field('command', 'Команда', { placeholder: 'npx' })}
                {field('args', 'Аргументы через пробел', { placeholder: '-y @modelcontextprotocol/server-filesystem /Users/…' })}
                {field('env', 'Переменные, по одной в строке: ИМЯ=значение', { area: true, placeholder: 'API_KEY=…' })}
                <p class="t-caption m-0 c-pill-yandex-fg">
                    Шлюз запустит эту команду на Mac как есть, без оболочки, в своей папке и только с этими переменными. Подключайте
                    программы, которым доверяете.
                </p>
                {error && (
                    <p role="alert" class="t-meta m-0 c-dot-offline">
                        {error}
                    </p>
                )}
            </Sheet>
        );
    }

    if (mode.kind === 'address') {
        const auth = (values.auth as 'none' | 'token' | 'oauth') || 'none';
        return (
            <Sheet
                title="Свой адрес"
                about="MCP-сервер в сети"
                icon={<ServerIcon transport="http" />}
                onClose={onClose}
                footer={footer('Подключить', !!value('url').trim() && (auth !== 'token' || !!value('token').trim()), () =>
                    void submit({
                        transport: 'http',
                        title: value('title').trim() || undefined,
                        url: value('url').trim(),
                        auth,
                        ...(auth === 'token' ? { token: value('token').trim() } : {}),
                        headers: pairs(value('headers'), ':'),
                    }),
                )}
            >
                {field('title', 'Название', { placeholder: 'Например, Мой сервер' })}
                {field('url', 'Адрес', { placeholder: 'https://example.org/mcp' })}
                <label class="flex flex-col gap-4">
                    <span class="t-caption">Вход</span>
                    <select class="field" value={auth} onChange={set('auth')}>
                        <option value="none">без входа</option>
                        <option value="token">токен (Bearer)</option>
                        <option value="oauth">через аккаунт (OAuth)</option>
                    </select>
                </label>
                {auth === 'token' && field('token', 'Токен', { secret: true })}
                {field('headers', 'Свои заголовки, по одному в строке: Имя: значение', { area: true, placeholder: 'X-Api-Key: …' })}
                {error && (
                    <p role="alert" class="t-meta m-0 c-dot-offline">
                        {error}
                    </p>
                )}
            </Sheet>
        );
    }

    return (
        <Sheet
            title="Вставить JSON"
            about="mcpServers из Claude Desktop или Cursor, servers из VS Code"
            onClose={onClose}
            footer={
                imported ? (
                    <button class="dash-done" onClick={() => onAdded(imported.added[0])}>
                        Готово
                    </button>
                ) : (
                    footer('Разобрать и добавить', !!value('json').trim(), async () => {
                        setBusy(true);
                        setError(null);
                        try {
                            setImported(await api.importMcp(value('json')));
                        } catch (e) {
                            setError(problemOf(e));
                        } finally {
                            setBusy(false);
                        }
                    })
                )
            }
        >
            {!imported && field('json', 'JSON', { area: true, placeholder: '{"mcpServers": {"name": {"command": "npx", "args": ["-y", "…"]}}}' })}
            {imported && (
                <div class="flex flex-col gap-6">
                    <p class="t-meta m-0 c-accent-text" role="status">
                        {imported.added.length ? `Добавлены: ${imported.added.map((server) => server.title).join(', ')}.` : 'Ничего не добавлено.'}{' '}
                        Их инструменты выключены — включите нужные в листе сервера.
                    </p>
                    {imported.problems.map((problem) => (
                        <p key={problem} class="t-caption m-0 c-pill-yandex-fg">
                            {problem}
                        </p>
                    ))}
                </div>
            )}
            {error && (
                <p role="alert" class="t-meta m-0 c-dot-offline">
                    {error}
                </p>
            )}
        </Sheet>
    );
}

/** Лист сервера: состояние, инструменты с переключателями, инструкция, подсказка, права, секреты. */
function ServerSheet({ server, onClose, onChanged }: { server: McpServer; onClose: () => void; onChanged: (fresh?: McpServer) => void }) {
    const mayManage = can('connectors.manage');
    const [busy, setBusy] = useState<string | null>(null);
    const [error, setError] = useState<string | null>(null);
    const [note, setNote] = useState(server.note);
    const [secretDraft, setSecretDraft] = useState<Record<string, string>>({});
    const [trying, setTrying] = useState<string | null>(null);
    const [args, setArgs] = useState('{}');
    const [tried, setTried] = useState<{ tool: string; content: string; isError: boolean; ms: number } | null>(null);
    /** Вход начат: страница сервиса открыта в новом окне; с телефона адрес после входа вставляют сюда. */
    const [loginOpen, setLoginOpen] = useState(false);
    const [returned, setReturned] = useState('');
    const [notice, setNotice] = useState<string | null>(null);

    const login = async () => {
        setBusy('login');
        setError(null);
        setNotice(null);
        // Окно открывается сразу, по нажатию: открытое позже браузер сочтёт всплывающим и закроет.
        const page = window.open('about:blank', '_blank');
        try {
            const result = await api.loginMcp(server.id);
            if (result.url) {
                if (page) page.location.href = result.url;
                else window.location.assign(result.url);
                setLoginOpen(true);
            } else {
                page?.close();
                setNotice('Вход уже есть — сервер подключается.');
                onChanged();
            }
        } catch (e) {
            page?.close();
            setError(problemOf(e));
        } finally {
            setBusy(null);
        }
    };

    const finishLogin = async () => {
        setBusy('finish');
        setError(null);
        try {
            const done = await api.completeMcpLogin(returned.trim());
            setNotice(`Вход в «${done.title}» выполнен — сервер подключается.`);
            setLoginOpen(false);
            setReturned('');
            onChanged();
        } catch (e) {
            setError(problemOf(e));
        } finally {
            setBusy(null);
        }
    };

    const act = async (key: string, work: () => Promise<McpServer | void>) => {
        setBusy(key);
        setError(null);
        try {
            const fresh = await work();
            onChanged(fresh || undefined);
        } catch (e) {
            setError(problemOf(e));
        } finally {
            setBusy(null);
        }
    };

    const tryTool = async (tool: McpTool) => {
        let parsed: Record<string, unknown>;
        try {
            parsed = JSON.parse(args || '{}');
        } catch {
            setError('Аргументы — не JSON.');
            return;
        }
        setBusy(`try:${tool.name}`);
        setError(null);
        try {
            setTried({ tool: tool.name, ...(await api.tryMcpTool(server.id, tool.name, parsed)) });
        } catch (e) {
            setError(problemOf(e));
        } finally {
            setBusy(null);
        }
    };

    const badge = (text: string, cls: string) => (
        <span key={text} class={`pill ${cls}`}>
            {text}
        </span>
    );
    const capabilities = CAPABILITIES.filter((entry) => ['smart_home', 'reminders', 'news', 'weather', 'calls', 'account', 'music'].includes(entry.name));

    return (
        <Sheet
            title={server.title}
            about={`${STATE[server.state].text} · ${whereOf(server)}`}
            icon={<ServerIcon brand={server.brand} transport={server.transport} preset={server.preset} />}
            onClose={onClose}
        >
            <div class="flex gap-6 flex-wrap">
                {badge(server.cloud ? 'через интернет' : 'дома', server.cloud ? 'pill-yandex' : 'pill-own')}
                {server.server && badge(`${server.server.name} ${server.server.version}`, 'pill-off')}
            </div>
            {server.detail && server.state !== 'ready' && (
                <p class={`t-meta m-0 ${server.state === 'login' ? 'c-pill-yandex-fg' : 'c-dot-offline'}`} role="status">
                    {server.state === 'login' ? 'Нужно войти в аккаунт сервиса.' : server.detail}
                </p>
            )}
            {server.state === 'error' && server.log.length > 0 && <p class="t-mono m-0 opacity-80">{server.log.join('\n')}</p>}
            {server.auth === 'oauth' && mayManage && (
                <div class="flex flex-col gap-6">
                    <div class="flex gap-6 flex-wrap items-center">
                        <Action label={server.state === 'login' ? 'войти' : 'войти заново'} tone={server.state === 'login' ? 'on' : 'off'} busy={busy === 'login'} onClick={() => void login()} />
                        <span class="t-caption">вход делаете вы сами на странице сервиса</span>
                    </div>
                    {loginOpen && (
                        <form
                            class="flex flex-col gap-6"
                            onSubmit={(e) => {
                                e.preventDefault();
                                void finishLogin();
                            }}
                        >
                            <span class="t-caption">
                                На этом Mac вход завершится сам. На телефоне после входа браузер покажет ошибку адреса localhost — скопируйте
                                адрес той страницы и вставьте сюда.
                            </span>
                            <div class="flex gap-8 flex-wrap">
                                <input
                                    class="field flex-1-1-220 min-w-0"
                                    aria-label="Адрес страницы после входа"
                                    placeholder="http://localhost/api/v1/mcp/oauth/callback?code=…"
                                    value={returned}
                                    onInput={(e) => setReturned((e.target as HTMLInputElement).value)}
                                />
                                <Action label="завершить вход" tone="on" disabled={!returned.trim()} busy={busy === 'finish'} onClick={() => void finishLogin()} />
                            </div>
                        </form>
                    )}
                    {notice && (
                        <p class="t-caption m-0 c-accent-text" role="status">
                            {notice}
                        </p>
                    )}
                </div>
            )}
            {mayManage && (
                <div class="flex gap-6 flex-wrap">
                    <Action label="переподключить" busy={busy === 'reconnect'} onClick={() => void act('reconnect', () => api.reconnectMcp(server.id))} />
                    <Action
                        label={server.enabled ? 'выключить' : 'включить'}
                        busy={busy === 'enabled'}
                        onClick={() => void act('enabled', () => api.updateMcpServer(server.id, { enabled: !server.enabled }))}
                    />
                    <Action
                        label="убрать"
                        busy={busy === 'remove'}
                        onClick={async () => {
                            const yes = await ask({ title: `Убрать «${server.title}»?`, text: 'Вместе с его ключами.', ok: 'Убрать', danger: true });
                            if (!yes) return;
                            void act('remove', async () => {
                                await api.removeMcpServer(server.id);
                                onClose();
                            });
                        }}
                    />
                </div>
            )}

            {server.tools.length > 0 && (
                <div class="flex flex-col">
                    <b class="my-4 mx-0">Инструменты</b>
                    {server.tools.map((tool) => (
                        <div key={tool.name} class="flex flex-col gap-6 py-8 px-0 bt-divider">
                            <div class="flex items-center gap-10">
                                <span class="flex-1 min-w-0 flex flex-col gap-2">
                                    <b class="flex gap-6 items-center flex-wrap fs-14">
                                        {tool.title}
                                        {tool.readOnly ? badge('только читает', 'pill-off') : badge('меняет данные', 'pill-yandex')}
                                        {tool.openWorld && badge('идёт в интернет', 'pill-yandex')}
                                    </b>
                                    <span class="t-caption line-clamp-2 overflow-hidden">
                                        {tool.description || tool.name}
                                    </span>
                                    {tool.changed && (
                                        <span class="t-caption c-pill-yandex-fg" role="status">
                                            Сервер поменял описание — инструмент выключен, пока вы не включите его снова.
                                        </span>
                                    )}
                                </span>
                                <Switch
                                    on={tool.on}
                                    label={`${tool.title}: ${tool.on ? 'включён' : 'выключен'}`}
                                    disabled={!mayManage || busy === `tool:${tool.name}` || server.state !== 'ready'}
                                    onChange={(on) => void act(`tool:${tool.name}`, () => api.setMcpTool(server.id, tool.name, { on }))}
                                />
                            </div>
                            {mayManage && server.state === 'ready' && (
                                <div class="flex gap-6 flex-wrap items-center">
                                    <Action
                                        label={trying === tool.name ? 'скрыть' : 'попробовать'}
                                        onClick={() => {
                                            setTrying(trying === tool.name ? null : tool.name);
                                            setTried(null);
                                            setArgs('{}');
                                        }}
                                    />
                                </div>
                            )}
                            {trying === tool.name && (
                                <div class="flex flex-col gap-6">
                                    <textarea
                                        class="field t-mono r-16 min-h-56"
                                        aria-label={`Аргументы ${tool.title}`}
                                        value={args}
                                        onInput={(e) => setArgs((e.target as HTMLTextAreaElement).value)}
                                    />
                                    <div>
                                        <Action label="вызвать" tone="on" busy={busy === `try:${tool.name}`} onClick={() => void tryTool(tool)} />
                                    </div>
                                    {tried?.tool === tool.name && (
                                        <p class={`t-mono m-0 ${tried.isError ? 'c-dot-offline' : 'c-text'}`} role="status">
                                            {`${tried.isError ? 'ошибка' : 'ответ'} за ${tried.ms} мс:\n${tried.content}`}
                                        </p>
                                    )}
                                </div>
                            )}
                        </div>
                    ))}
                </div>
            )}

            {server.instructions.text && (
                <div class="flex flex-col gap-6">
                    <b class="flex gap-8 items-center">
                        Инструкция сервера
                        {server.instructions.approved ? badge('в подсказке модели', 'pill-own') : badge('ждёт одобрения', 'pill-yandex')}
                    </b>
                    <p class="t-caption m-0 max-h-120 overflow-auto pre-wrap">
                        {server.instructions.text}
                    </p>
                    {mayManage && !server.instructions.approved && (
                        <div>
                            <Action label="одобрить" tone="on" busy={busy === 'approve'} onClick={() => void act('approve', () => api.approveMcpInstructions(server.id))} />
                        </div>
                    )}
                </div>
            )}

            <div class="flex flex-col gap-6">
                <b>Подсказка модели</b>
                <textarea
                    class="field r-16 min-h-64 resize-y"
                    aria-label="Подсказка модели"
                    placeholder="Например: «Это наш общий список дел; покупки — в проект „Покупки“»"
                    disabled={!mayManage}
                    value={note}
                    onInput={(e) => setNote((e.target as HTMLTextAreaElement).value)}
                />
                {mayManage && note !== server.note && (
                    <div>
                        <Action label="сохранить" tone="on" busy={busy === 'note'} onClick={() => void act('note', () => api.updateMcpServer(server.id, { note }))} />
                    </div>
                )}
            </div>

            {mayManage && (
                <div class="flex flex-col">
                    <b class="my-4 mx-0">Кто и как пользуется</b>
                    {(
                        [
                            ['voice', 'Голосом можно', 'колонку слышит любой рядом — выключите для личного', server.voice],
                            ['adminOnly', 'Только администратору', 'домашние не увидят его в проверочном чате', server.adminOnly],
                            ['outside', 'Ответы из интернета', 'после них модель в этом ответе ничего не меняет', server.outside],
                        ] as const
                    ).map(([key, title, sub, on]) => (
                        <div key={key} class="flex items-center gap-10 py-8 px-0 bt-divider">
                            <span class="flex-1 flex flex-col gap-2">
                                <b class="fs-14">{title}</b>
                                <span class="t-caption">{sub}</span>
                            </span>
                            <Switch on={on} label={title} disabled={busy === key} onChange={(next) => void act(key, () => api.updateMcpServer(server.id, { [key]: next }))} />
                        </div>
                    ))}
                    <label class="flex items-center gap-10 py-8 px-0 bt-divider flex-wrap">
                        <span class="flex-1 flex flex-col gap-2 min-w-180">
                            <b class="fs-14">Это возможность колонки</b>
                            <span class="t-caption">тогда действуют её маршрут и запреты</span>
                        </span>
                        <select
                            class="field"
                            value={server.capability ?? ''}
                            onChange={(e) => {
                                const capability = (e.target as HTMLSelectElement).value || null;
                                void act('capability', () => api.updateMcpServer(server.id, { capability }));
                            }}
                        >
                            <option value="">нет</option>
                            {capabilities.map((entry) => (
                                <option key={entry.name} value={entry.name}>
                                    {entry.title}
                                </option>
                            ))}
                        </select>
                    </label>
                </div>
            )}

            {mayManage && server.secrets.length > 0 && (
                <div class="flex flex-col">
                    <b class="my-4 mx-0">Ключи и переменные</b>
                    {server.secrets.map((secret) => {
                        const key = `${secret.kind}:${secret.name}`;
                        const save = () =>
                            void act(key, () =>
                                api.updateMcpServer(
                                    server.id,
                                    secret.kind === 'token'
                                        ? { token: secretDraft[key] ?? '' }
                                        : secret.kind === 'env'
                                          ? { env: { [secret.name]: secretDraft[key] ?? '' } }
                                          : { headers: { [secret.name]: secretDraft[key] ?? '' } },
                                ),
                            );
                        return (
                            <form
                                key={key}
                                class="flex items-center gap-8 py-8 px-0 bt-divider flex-wrap"
                                onSubmit={(e) => {
                                    e.preventDefault();
                                    save();
                                }}
                            >
                                <span class="flex-1 min-w-140 flex flex-col">
                                    <b class="fs-14">{secret.kind === 'token' ? 'Токен' : secret.name}</b>
                                    <span class="t-caption">{secret.set ? `задан · ${secret.hint ?? ''}` : 'не задан'}</span>
                                </span>
                                <input
                                    class="field flex-1-1-160 min-w-0"
                                    type="password"
                                    autocomplete="off"
                                    aria-label={`Новое значение ${secret.name}`}
                                    placeholder="новое значение"
                                    value={secretDraft[key] ?? ''}
                                    onInput={(e) => setSecretDraft((all) => ({ ...all, [key]: (e.target as HTMLInputElement).value }))}
                                />
                                <Action label="заменить" disabled={!(secretDraft[key] ?? '').trim()} busy={busy === key} onClick={save} />
                            </form>
                        );
                    })}
                </div>
            )}
            {error && (
                <p role="alert" class="t-meta m-0 c-dot-offline">
                    {error}
                </p>
            )}
        </Sheet>
    );
}
