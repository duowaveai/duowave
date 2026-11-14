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
import { byMode } from '../words';
import { tr } from '../i18n';

const states = (): Record<McpState, { text: string; cls: string }> => ({
    ready: { text: tr.mcp.stateReady, cls: 'pill-own' },
    connecting: { text: tr.mcp.stateConnecting, cls: 'pill-off' },
    login: { text: tr.mcp.stateLogin, cls: 'pill-yandex' },
    error: { text: tr.mcp.stateError, cls: 'pill-error' },
    off: { text: tr.mcp.stateOff, cls: 'pill-off' },
});

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
                    {tr.mcp.title}
                </h2>
                <div class="flex gap-6 flex-wrap">
                    {can('mcp.use') && <Action label={tr.mcp.askBrain} onClick={onAsk} />}
                    {mayManage && <Action label={tr.mcp.add} tone="on" onClick={() => setAdding(true)} />}
                </div>
            </div>
            {error && (
                <p class="t-caption c-dot-offline m-0 mb-8" role="alert">
                    {error}
                </p>
            )}
            {view && view.toolsOn > view.toolsLimit && (
                <p class="t-caption c-pill-yandex-fg m-0 mb-8" role="status">
                    {tr.mcp.toolsOverLimit(view.toolsOn, view.toolsLimit)}
                </p>
            )}
            {view && !view.servers.length && (
                <p class="t-caption m-0 mb-8">
                    {tr.mcp.emptyHint}
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
                                <span class={`pill ${server.cloud ? 'pill-yandex' : 'pill-own'}`}>{server.cloud ? tr.mcp.viaInternet : tr.mcp.atHome}</span>
                            </b>
                            <span class="t-caption">
                                {server.state === 'ready'
                                    ? `${tr.mcp.toolsSummary(on, server.tools.length)}${changed ? tr.mcp.toolsChanged(changed) : ''}${
                                          server.instructions.text && !server.instructions.approved ? tr.mcp.instructionsWait : ''
                                      }`
                                    : (server.detail ?? whereOf(server))}
                            </span>
                        </span>
                        <span class={`pill ${states()[server.state].cls}`}>{states()[server.state].text}</span>
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
                {busy ? tr.mcp.connecting : label}
            </button>
            <button class="pill pill-off dash-confirm-btn" onClick={() => (setMode({ kind: 'pick' }), setError(null), setImported(null))}>
                {tr.mcp.back}
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
            <Sheet title={tr.mcp.addTitle} about={tr.mcp.addAbout} onClose={onClose}>
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
                    {tile('program', <ServerIcon transport="stdio" />, tr.mcp.ownProgram, tr.mcp.ownProgramText, () => setMode({ kind: 'program' }))}
                    {tile('address', <ServerIcon transport="http" />, tr.mcp.ownAddress, tr.mcp.ownAddressText, () => setMode({ kind: 'address' }))}
                    {tile(
                        'json',
                        <span class="route-icon w-38 h-38 flex-none" aria-hidden="true">
                            <IconBraces size={19} />
                        </span>,
                        tr.mcp.pasteJson,
                        tr.mcp.pasteJsonText,
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
                about={preset.transport === 'stdio' ? tr.mcp.aboutProgramMac : preset.outside ? tr.mcp.aboutServerInternet : tr.mcp.aboutServerAddress}
                icon={<ServerIcon brand={preset.brand} transport={preset.transport} preset={preset.id} />}
                onClose={onClose}
                footer={footer(tr.mcp.connect, !missing, () =>
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
                {needsUrl && field('url', tr.mcp.haAddress, { placeholder: 'http://homeassistant.local:8123' })}
                {preset.fields.map((entry) => (
                    <div key={entry.name} class="flex flex-col gap-4">
                        {field(entry.name, entry.title, { secret: entry.secret, placeholder: entry.placeholder })}
                        {entry.url && (
                            <a class="t-caption c-accent-text" href={entry.url} target="_blank" rel="noopener noreferrer">
                                {tr.mcp.whereToGet}
                            </a>
                        )}
                    </div>
                ))}
                {preset.id === 'homeassistant' && (
                    <p class="t-caption m-0">
                        {tr.mcp.haSetup}
                    </p>
                )}
                {preset.auth === 'oauth' && (
                    <p class="t-caption m-0">
                        {tr.mcp.oauthNote}
                    </p>
                )}
                {preset.install && (
                    <p class="t-caption m-0">
                        {tr.mcp.firstRunDownload(preset.install.package, preset.install.runner)}
                    </p>
                )}
                {field('note', tr.mcp.modelNoteField, { area: true })}
                <p class="t-caption m-0">
                    {tr.mcp.secretsNote}
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
                title={tr.mcp.ownProgram}
                about={tr.mcp.programAbout}
                icon={<ServerIcon transport="stdio" />}
                onClose={onClose}
                footer={footer(tr.mcp.connect, !!value('command').trim(), () =>
                    void submit({
                        transport: 'stdio',
                        title: value('title').trim() || undefined,
                        command: value('command').trim(),
                        args: value('args').trim(),
                        env: pairs(value('env'), '='),
                    }),
                )}
            >
                {field('title', tr.mcp.fieldName, { placeholder: tr.mcp.programNamePlaceholder })}
                {field('command', tr.mcp.fieldCommand, { placeholder: 'npx' })}
                {field('args', tr.mcp.fieldArgs, { placeholder: '-y @modelcontextprotocol/server-filesystem /Users/…' })}
                {field('env', tr.mcp.fieldEnv, { area: true, placeholder: 'API_KEY=…' })}
                <p class="t-caption m-0 c-pill-yandex-fg">
                    {tr.mcp.programWarning}
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
                title={tr.mcp.ownAddress}
                about={tr.mcp.addressAbout}
                icon={<ServerIcon transport="http" />}
                onClose={onClose}
                footer={footer(tr.mcp.connect, !!value('url').trim() && (auth !== 'token' || !!value('token').trim()), () =>
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
                {field('title', tr.mcp.fieldName, { placeholder: tr.mcp.addressNamePlaceholder })}
                {field('url', tr.mcp.fieldAddress, { placeholder: 'https://example.org/mcp' })}
                <label class="flex flex-col gap-4">
                    <span class="t-caption">{tr.mcp.fieldLogin}</span>
                    <select class="field" value={auth} onChange={set('auth')}>
                        <option value="none">{tr.mcp.authNone}</option>
                        <option value="token">{tr.mcp.authToken}</option>
                        <option value="oauth">{tr.mcp.authOauth}</option>
                    </select>
                </label>
                {auth === 'token' && field('token', tr.mcp.fieldToken, { secret: true })}
                {field('headers', tr.mcp.fieldHeaders, { area: true, placeholder: 'X-Api-Key: …' })}
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
            title={tr.mcp.jsonTitle}
            about={tr.mcp.jsonAbout}
            onClose={onClose}
            footer={
                imported ? (
                    <button class="dash-done" onClick={() => onAdded(imported.added[0])}>
                        {tr.mcp.done}
                    </button>
                ) : (
                    footer(tr.mcp.parseAndAdd, !!value('json').trim(), async () => {
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
                        {imported.added.length ? tr.mcp.importedAdded(imported.added.map((server) => server.title).join(', ')) : tr.mcp.importedNothing}{' '}
                        {tr.mcp.importedToolsOff}
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
                setNotice(tr.mcp.loginExists);
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
            setNotice(tr.mcp.loginDone(done.title));
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
            setError(tr.mcp.argsNotJson);
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
            about={`${states()[server.state].text} · ${whereOf(server)}`}
            icon={<ServerIcon brand={server.brand} transport={server.transport} preset={server.preset} />}
            onClose={onClose}
        >
            <div class="flex gap-6 flex-wrap">
                {badge(server.cloud ? tr.mcp.viaInternet : tr.mcp.atHome, server.cloud ? 'pill-yandex' : 'pill-own')}
                {server.server && badge(tr.mcp.versionBadge(server.server.name, server.server.version), 'pill-off')}
            </div>
            {server.detail && server.state !== 'ready' && (
                <p class={`t-meta m-0 ${server.state === 'login' ? 'c-pill-yandex-fg' : 'c-dot-offline'}`} role="status">
                    {server.state === 'login' ? tr.mcp.needLogin : server.detail}
                </p>
            )}
            {server.state === 'error' && server.log.length > 0 && <p class="t-mono m-0 opacity-80">{server.log.join('\n')}</p>}
            {server.auth === 'oauth' && mayManage && (
                <div class="flex flex-col gap-6">
                    <div class="flex gap-6 flex-wrap items-center">
                        <Action label={server.state === 'login' ? tr.mcp.login : tr.mcp.loginAgain} tone={server.state === 'login' ? 'on' : 'off'} busy={busy === 'login'} onClick={() => void login()} />
                        <span class="t-caption">{tr.mcp.loginByYou}</span>
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
                                {tr.mcp.loginPhoneNote}
                            </span>
                            <div class="flex gap-8 flex-wrap">
                                <input
                                    class="field flex-1-1-220 min-w-0"
                                    aria-label={tr.mcp.returnedLabel}
                                    placeholder="http://localhost/api/v1/mcp/oauth/callback?code=…"
                                    value={returned}
                                    onInput={(e) => setReturned((e.target as HTMLInputElement).value)}
                                />
                                <Action label={tr.mcp.finishLogin} tone="on" disabled={!returned.trim()} busy={busy === 'finish'} onClick={() => void finishLogin()} />
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
                    <Action label={tr.mcp.reconnect} busy={busy === 'reconnect'} onClick={() => void act('reconnect', () => api.reconnectMcp(server.id))} />
                    <Action
                        label={server.enabled ? tr.mcp.disable : tr.mcp.enable}
                        busy={busy === 'enabled'}
                        onClick={() => void act('enabled', () => api.updateMcpServer(server.id, { enabled: !server.enabled }))}
                    />
                    <Action
                        label={tr.mcp.remove}
                        busy={busy === 'remove'}
                        onClick={async () => {
                            const yes = await ask({ title: tr.mcp.removeTitle(server.title), text: tr.mcp.removeText, ok: tr.mcp.removeOk, danger: true });
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
                    <b class="my-4 mx-0">{tr.mcp.tools}</b>
                    {server.tools.map((tool) => (
                        <div key={tool.name} class="flex flex-col gap-6 py-8 px-0 bt-divider">
                            <div class="flex items-center gap-10">
                                <span class="flex-1 min-w-0 flex flex-col gap-2">
                                    <b class="flex gap-6 items-center flex-wrap fs-14">
                                        {tool.title}
                                        {tool.readOnly ? badge(tr.mcp.readOnly, 'pill-off') : badge(tr.mcp.changesData, 'pill-yandex')}
                                        {tool.openWorld && badge(tr.mcp.goesOnline, 'pill-yandex')}
                                    </b>
                                    <span class="t-caption line-clamp-2 overflow-hidden">
                                        {tool.description || tool.name}
                                    </span>
                                    {tool.changed && (
                                        <span class="t-caption c-pill-yandex-fg" role="status">
                                            {tr.mcp.descriptionChanged}
                                        </span>
                                    )}
                                </span>
                                <Switch
                                    on={tool.on}
                                    label={tr.mcp.toolSwitch(tool.title, tool.on)}
                                    disabled={!mayManage || busy === `tool:${tool.name}` || server.state !== 'ready'}
                                    onChange={(on) => void act(`tool:${tool.name}`, () => api.setMcpTool(server.id, tool.name, { on }))}
                                />
                            </div>
                            {mayManage && server.state === 'ready' && (
                                <div class="flex gap-6 flex-wrap items-center">
                                    <Action
                                        label={trying === tool.name ? tr.mcp.hide : tr.mcp.tryIt}
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
                                        aria-label={tr.mcp.argsLabel(tool.title)}
                                        value={args}
                                        onInput={(e) => setArgs((e.target as HTMLTextAreaElement).value)}
                                    />
                                    <div>
                                        <Action label={tr.mcp.call} tone="on" busy={busy === `try:${tool.name}`} onClick={() => void tryTool(tool)} />
                                    </div>
                                    {tried?.tool === tool.name && (
                                        <p class={`t-mono m-0 ${tried.isError ? 'c-dot-offline' : 'c-text'}`} role="status">
                                            {tr.mcp.triedResult(tried.isError, tried.ms, tried.content)}
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
                        {tr.mcp.instructionsTitle}
                        {server.instructions.approved ? badge(tr.mcp.inPrompt, 'pill-own') : badge(tr.mcp.awaitsApproval, 'pill-yandex')}
                    </b>
                    <p class="t-caption m-0 max-h-120 overflow-auto pre-wrap">
                        {server.instructions.text}
                    </p>
                    {mayManage && !server.instructions.approved && (
                        <div>
                            <Action label={tr.mcp.approve} tone="on" busy={busy === 'approve'} onClick={() => void act('approve', () => api.approveMcpInstructions(server.id))} />
                        </div>
                    )}
                </div>
            )}

            <div class="flex flex-col gap-6">
                <b>{tr.mcp.modelNote}</b>
                <textarea
                    class="field r-16 min-h-64 resize-y"
                    aria-label={tr.mcp.modelNote}
                    placeholder={tr.mcp.modelNotePlaceholder}
                    disabled={!mayManage}
                    value={note}
                    onInput={(e) => setNote((e.target as HTMLTextAreaElement).value)}
                />
                {mayManage && note !== server.note && (
                    <div>
                        <Action label={tr.mcp.save} tone="on" busy={busy === 'note'} onClick={() => void act('note', () => api.updateMcpServer(server.id, { note }))} />
                    </div>
                )}
            </div>

            {mayManage && (
                <div class="flex flex-col">
                    <b class="my-4 mx-0">{tr.mcp.whoUses}</b>
                    {(
                        [
                            ['voice', tr.mcp.voiceAllowed, byMode(tr.mcp.voiceAllowedSpeaker, tr.mcp.voiceAllowedMic), server.voice],
                            ['adminOnly', tr.mcp.adminOnly, tr.mcp.adminOnlySub, server.adminOnly],
                            ['outside', tr.mcp.outsideAnswers, tr.mcp.outsideAnswersSub, server.outside],
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
                            <b class="fs-14">{byMode(tr.mcp.isCapabilitySpeaker, tr.mcp.isCapabilityOwn)}</b>
                            <span class="t-caption">{tr.mcp.capabilitySub}</span>
                        </span>
                        <select
                            class="field"
                            value={server.capability ?? ''}
                            onChange={(e) => {
                                const capability = (e.target as HTMLSelectElement).value || null;
                                void act('capability', () => api.updateMcpServer(server.id, { capability }));
                            }}
                        >
                            <option value="">{tr.mcp.none}</option>
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
                    <b class="my-4 mx-0">{tr.mcp.secretsTitle}</b>
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
                                    <b class="fs-14">{secret.kind === 'token' ? tr.mcp.fieldToken : secret.name}</b>
                                    <span class="t-caption">{secret.set ? tr.mcp.secretSet(secret.hint ?? '') : tr.mcp.secretNotSet}</span>
                                </span>
                                <input
                                    class="field flex-1-1-160 min-w-0"
                                    type="password"
                                    autocomplete="off"
                                    aria-label={tr.mcp.newValueLabel(secret.name)}
                                    placeholder={tr.mcp.newValue}
                                    value={secretDraft[key] ?? ''}
                                    onInput={(e) => setSecretDraft((all) => ({ ...all, [key]: (e.target as HTMLInputElement).value }))}
                                />
                                <Action label={tr.mcp.replace} disabled={!(secretDraft[key] ?? '').trim()} busy={busy === key} onClick={save} />
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
