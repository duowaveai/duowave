import { useEffect, useState } from 'preact/hooks';

import { McpPreset, McpServer, McpServerInput, McpState, McpTool, McpView, api, can, problemOf } from '../api';
import { BRANDS, type BrandId } from '../brand-icons';
import { CAPABILITIES } from '../capabilities';
import { IconBraces, IconGlobe, IconPage, IconTerminal } from '../icons';
import { BrandMark } from '../ui/brand';
import { Action } from '../ui/cards';
import { SaveBar, Sheet, WinRow, WinSection } from '../ui/sheet';
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

/** Слово с большой буквы — для кнопок, у которых строка словаря строчная (она же — в строках карточки). */
const cap = (text: string): string => text.charAt(0).toUpperCase() + text.slice(1);

/** Крутилка в занятой кнопке. */
const spin = (on: boolean) => on && <span class="spinner spinner-sm" aria-hidden="true" />;

/** Окно «Добавить сервер»: готовые (со знаками), своя программа, свой адрес, вставка JSON. */
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
        <label key={name} class="flex flex-col gap-6">
            <span class="win-label">{title}</span>
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

    // Нижняя полоса шага: «Назад» к выбору и одно главное справа.
    const footer = (label: string, ready: boolean, go: () => void) => (
        <>
            <button class="btn btn-plain" onClick={() => (setMode({ kind: 'pick' }), setError(null), setImported(null))}>
                {tr.mcp.back}
            </button>
            <button class="btn btn-main" disabled={busy || !ready} aria-busy={busy || undefined} onClick={go}>
                {spin(busy)}
                {busy ? tr.mcp.connecting : label}
            </button>
        </>
    );

    const problem = error && (
        <p role="alert" class="win-callout m-0">
            {error}
        </p>
    );

    if (mode.kind === 'pick') {
        const tile = (key: string, icon: preact.ComponentChildren, title: string, text: string, pick: () => void, disabled = false) => (
            <button key={key} class={`win-row win-choice text-left${disabled ? ' opacity-50' : ''}`} disabled={disabled} onClick={pick}>
                {icon}
                <span class="win-row-main">
                    <span class="win-row-t">{title}</span>
                    <span class="win-row-s">{text}</span>
                </span>
            </button>
        );
        return (
            <Sheet title={tr.mcp.addTitle} about={tr.mcp.addAbout} onClose={onClose}>
                {presets.length > 0 && (
                    <WinSection title={tr.mcp.presetsSection} group>
                        {presets.map((preset) =>
                            tile(
                                preset.id,
                                <ServerIcon brand={preset.brand} transport={preset.transport} preset={preset.id} size={32} />,
                                preset.title,
                                preset.ready ? preset.description : (preset.why ?? preset.description),
                                () => (setValues({ note: preset.note ?? '' }), setMode({ kind: 'preset', preset })),
                            ),
                        )}
                    </WinSection>
                )}
                <WinSection title={tr.mcp.ownSection} group>
                    {tile('program', <ServerIcon transport="stdio" size={32} />, tr.mcp.ownProgram, tr.mcp.ownProgramText, () => setMode({ kind: 'program' }))}
                    {tile('address', <ServerIcon transport="http" size={32} />, tr.mcp.ownAddress, tr.mcp.ownAddressText, () => setMode({ kind: 'address' }))}
                    {tile(
                        'json',
                        <span class="route-icon w-32 h-32 flex-none" aria-hidden="true">
                            <IconBraces size={16} />
                        </span>,
                        tr.mcp.pasteJson,
                        tr.mcp.pasteJsonText,
                        () => setMode({ kind: 'json' }),
                    )}
                </WinSection>
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
                icon={<ServerIcon brand={preset.brand} transport={preset.transport} preset={preset.id} size={44} />}
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
                <p class="win-lead">{preset.description}</p>
                {preset.id === 'homeassistant' && <p class="win-callout is-calm m-0">{tr.mcp.haSetup}</p>}
                {needsUrl && field('url', tr.mcp.haAddress, { placeholder: 'http://homeassistant.local:8123' })}
                {preset.fields.map((entry) => (
                    <div key={entry.name} class="flex flex-col gap-6">
                        {field(entry.name, entry.title, { secret: entry.secret, placeholder: entry.placeholder })}
                        {entry.url && (
                            <a class="win-hint c-accent-text self-start" href={entry.url} target="_blank" rel="noopener noreferrer">
                                {tr.mcp.whereToGet}
                            </a>
                        )}
                    </div>
                ))}
                {preset.auth === 'oauth' && <p class="win-note">{tr.mcp.oauthNote}</p>}
                {preset.install && <p class="win-note">{tr.mcp.firstRunDownload(preset.install.package, preset.install.runner)}</p>}
                {field('note', tr.mcp.modelNoteField, { area: true })}
                <p class="win-note">{tr.mcp.secretsNote}</p>
                {problem}
            </Sheet>
        );
    }

    if (mode.kind === 'program') {
        return (
            <Sheet
                title={tr.mcp.ownProgram}
                about={tr.mcp.programAbout}
                icon={<ServerIcon transport="stdio" size={44} />}
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
                <p class="win-callout m-0">{tr.mcp.programWarning}</p>
                {field('title', tr.mcp.fieldName, { placeholder: tr.mcp.programNamePlaceholder })}
                {field('command', tr.mcp.fieldCommand, { placeholder: 'npx' })}
                {field('args', tr.mcp.fieldArgs, { placeholder: '-y @modelcontextprotocol/server-filesystem /Users/…' })}
                {field('env', tr.mcp.fieldEnv, { area: true, placeholder: 'API_KEY=…' })}
                {problem}
            </Sheet>
        );
    }

    if (mode.kind === 'address') {
        const auth = (values.auth as 'none' | 'token' | 'oauth') || 'none';
        return (
            <Sheet
                title={tr.mcp.ownAddress}
                about={tr.mcp.addressAbout}
                icon={<ServerIcon transport="http" size={44} />}
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
                <label class="flex flex-col gap-6">
                    <span class="win-label">{tr.mcp.fieldLogin}</span>
                    <select class="field" value={auth} onChange={set('auth')}>
                        <option value="none">{tr.mcp.authNone}</option>
                        <option value="token">{tr.mcp.authToken}</option>
                        <option value="oauth">{tr.mcp.authOauth}</option>
                    </select>
                </label>
                {auth === 'token' && field('token', tr.mcp.fieldToken, { secret: true })}
                {field('headers', tr.mcp.fieldHeaders, { area: true, placeholder: 'X-Api-Key: …' })}
                {problem}
            </Sheet>
        );
    }

    // Разобрали: «Готово» открывает первый добавленный сервер; не добавилось ничего — окно закрывают крестиком.
    return (
        <Sheet
            title={tr.mcp.jsonTitle}
            about={tr.mcp.jsonAbout}
            icon={
                <span class="route-icon flex-none" aria-hidden="true" style={{ width: 44, height: 44 }}>
                    <IconBraces size={22} />
                </span>
            }
            onClose={onClose}
            footer={
                imported ? (
                    imported.added.length > 0 ? (
                        <button class="btn btn-main" onClick={() => onAdded(imported.added[0])}>
                            {tr.mcp.done}
                        </button>
                    ) : undefined
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
                <>
                    <p class="win-lead" role="status">
                        {imported.added.length ? tr.mcp.importedAdded(imported.added.map((server) => server.title).join(', ')) : tr.mcp.importedNothing}{' '}
                        {tr.mcp.importedToolsOff}
                    </p>
                    {imported.problems.map((problem) => (
                        <p key={problem} class="win-callout m-0">
                            {problem}
                        </p>
                    ))}
                </>
            )}
            {problem}
        </Sheet>
    );
}

type ServerTab = 'tools' | 'settings';

/**
 * Окно сервера: шапка — состояние, где он, версия; «Инструменты» — вход, инструкция сервера, инструменты с
 * переключателями; «Настройки» — подсказка модели (её «Сохранить» — в нижней полосе), кто пользуется, ключи, «Опасное».
 * Включить выключенный и войти — главная кнопка внизу; переподключить и войти заново — за «⋯».
 */
function ServerSheet({ server, onClose, onChanged }: { server: McpServer; onClose: () => void; onChanged: (fresh?: McpServer) => void }) {
    const mayManage = can('connectors.manage');
    const [tab, setTab] = useState<ServerTab>('tools');
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

    const remove = async () => {
        const yes = await ask({ title: tr.mcp.removeTitle(server.title), text: tr.mcp.removeText, ok: tr.mcp.removeOk, danger: true });
        if (!yes) return;
        void act('remove', async () => {
            await api.removeMcpServer(server.id);
            onClose();
        });
    };

    const badge = (text: string, cls: string) => (
        <span key={text} class={`pill ${cls}`}>
            {text}
        </span>
    );
    const capabilities = CAPABILITIES.filter((entry) => ['smart_home', 'reminders', 'news', 'weather', 'calls', 'account', 'music'].includes(entry.name));
    const oauth = server.auth === 'oauth' && mayManage;
    const toolsOn = server.tools.filter((tool) => tool.on).length;
    const noteDirty = mayManage && note !== server.note;

    const tabs = [
        { id: 'tools' as const, label: tr.mcp.tools },
        { id: 'settings' as const, label: tr.plugins.tabSettings },
    ];
    const menu = mayManage
        ? [
              { label: cap(tr.mcp.reconnect), disabled: busy !== null, act: () => void act('reconnect', () => api.reconnectMcp(server.id)) },
              ...(oauth && server.state !== 'login' ? [{ label: cap(tr.mcp.loginAgain), disabled: busy !== null, act: () => void login() }] : []),
          ]
        : undefined;

    // Нижняя полоса: на «Настройках» с изменённой подсказкой — «Сохранить»; иначе — войти или включить, если нужно.
    const footer =
        tab === 'settings' && (noteDirty || busy === 'note') ? (
            <SaveBar
                dirty={noteDirty}
                busy={busy === 'note'}
                onRevert={() => setNote(server.note)}
                onSave={() => void act('note', () => api.updateMcpServer(server.id, { note }))}
            />
        ) : oauth && server.state === 'login' ? (
            <button class="btn btn-main" disabled={busy !== null} aria-busy={busy === 'login' || undefined} onClick={() => void login()}>
                {spin(busy === 'login')}
                {cap(tr.mcp.login)}
            </button>
        ) : mayManage && !server.enabled ? (
            <button
                class="btn btn-main"
                disabled={busy !== null}
                aria-busy={busy === 'enabled' || undefined}
                onClick={() => void act('enabled', () => api.updateMcpServer(server.id, { enabled: true }))}
            >
                {spin(busy === 'enabled')}
                {cap(tr.mcp.enable)}
            </button>
        ) : undefined;

    return (
        <Sheet
            title={server.title}
            about={whereOf(server)}
            meta={
                <>
                    <span class={`pill ${states()[server.state].cls}`}>{states()[server.state].text}</span>
                    {badge(server.cloud ? tr.mcp.viaInternet : tr.mcp.atHome, server.cloud ? 'pill-yandex' : 'pill-own')}
                    {server.server && <span class="win-chip">{tr.mcp.versionBadge(server.server.name, server.server.version)}</span>}
                </>
            }
            icon={<ServerIcon brand={server.brand} transport={server.transport} preset={server.preset} size={44} />}
            menu={menu}
            tabs={tabs}
            tab={tab}
            onTab={setTab}
            onClose={onClose}
            footer={footer}
        >
            {error && (
                <p role="alert" class="win-callout m-0">
                    {error}
                </p>
            )}
            {tab === 'tools' && (
                <>
                    {server.detail && server.state !== 'ready' && (
                        <p class="win-callout m-0" role="status">
                            {server.state === 'login' ? (oauth ? `${tr.mcp.needLogin} ${cap(tr.mcp.loginByYou)}.` : tr.mcp.needLogin) : server.detail}
                        </p>
                    )}
                    {server.state === 'error' && server.log.length > 0 && <p class="win-callout is-calm m-0 t-mono">{server.log.join('\n')}</p>}
                    {oauth && loginOpen && (
                        <WinSection group>
                            <form
                                class="win-row is-col"
                                onSubmit={(e) => {
                                    e.preventDefault();
                                    void finishLogin();
                                }}
                            >
                                <span class="win-row-s">{tr.mcp.loginPhoneNote}</span>
                                <div class="flex gap-8 flex-wrap items-center">
                                    <input
                                        class="field flex-1-1-220 min-w-0"
                                        aria-label={tr.mcp.returnedLabel}
                                        placeholder="http://localhost/api/v1/mcp/oauth/callback?code=…"
                                        value={returned}
                                        onInput={(e) => setReturned((e.target as HTMLInputElement).value)}
                                    />
                                    <button type="submit" class="btn btn-plain btn-sm" disabled={!returned.trim() || busy === 'finish'} aria-busy={busy === 'finish' || undefined}>
                                        {spin(busy === 'finish')}
                                        {cap(tr.mcp.finishLogin)}
                                    </button>
                                </div>
                            </form>
                        </WinSection>
                    )}
                    {oauth && notice && (
                        <p class="win-callout is-calm m-0" role="status">
                            {notice}
                        </p>
                    )}

                    {server.instructions.text && (
                        <WinSection
                            title={tr.mcp.instructionsTitle}
                            aside={server.instructions.approved ? badge(tr.mcp.inPrompt, 'pill-own') : badge(tr.mcp.awaitsApproval, 'pill-yandex')}
                            group
                        >
                            <div class="win-row is-col">
                                <p class="win-text m-0 max-h-120 overflow-auto pre-wrap">{server.instructions.text}</p>
                                {mayManage && !server.instructions.approved && (
                                    <button
                                        class="btn btn-plain btn-sm self-start"
                                        disabled={busy === 'approve'}
                                        aria-busy={busy === 'approve' || undefined}
                                        onClick={() => void act('approve', () => api.approveMcpInstructions(server.id))}
                                    >
                                        {spin(busy === 'approve')}
                                        {cap(tr.mcp.approve)}
                                    </button>
                                )}
                            </div>
                        </WinSection>
                    )}

                    {server.tools.length > 0 && (
                        <WinSection title={tr.mcp.tools} aside={tr.mcp.toolsSummary(toolsOn, server.tools.length)} group>
                            {server.tools.map((tool) => (
                                <div key={tool.name} class="win-row is-col">
                                    <div class="flex items-center gap-12">
                                        <div class="win-row-main">
                                            <span class="win-row-t">{tool.title}</span>
                                            <span class="win-row-s line-clamp-2 overflow-hidden">{tool.description || tool.name}</span>
                                        </div>
                                        <Switch
                                            on={tool.on}
                                            label={tr.mcp.toolSwitch(tool.title, tool.on)}
                                            disabled={!mayManage || busy === `tool:${tool.name}` || server.state !== 'ready'}
                                            onChange={(on) => void act(`tool:${tool.name}`, () => api.setMcpTool(server.id, tool.name, { on }))}
                                        />
                                    </div>
                                    <div class="win-tags">
                                        {tool.readOnly ? badge(tr.mcp.readOnly, 'pill-off') : badge(tr.mcp.changesData, 'pill-yandex')}
                                        {tool.openWorld && badge(tr.mcp.goesOnline, 'pill-yandex')}
                                        {tool.background && badge(tr.mcp.inBackground, 'pill-own')}
                                        {tool.paid && badge(tr.mcp.paidRun, 'pill-yandex')}
                                    </div>
                                    {tool.changed && (
                                        <span class="win-row-s c-pill-yandex-fg" role="status">
                                            {tr.mcp.descriptionChanged}
                                        </span>
                                    )}
                                    {mayManage && server.state === 'ready' && (
                                        <div class="flex gap-6 flex-wrap items-center" title={tr.mcp.backgroundHint}>
                                            <Action
                                                on={tool.background}
                                                label={tr.mcp.backgroundToggle(tool.background)}
                                                busy={busy === `bg:${tool.name}`}
                                                onClick={() =>
                                                    void act(`bg:${tool.name}`, () =>
                                                        api.setMcpTool(server.id, tool.name, { background: !tool.background }),
                                                    )
                                                }
                                            />
                                            <Action
                                                on={tool.paid}
                                                label={tr.mcp.paidToggle(tool.paid)}
                                                busy={busy === `paid:${tool.name}`}
                                                onClick={() =>
                                                    void act(`paid:${tool.name}`, () => api.setMcpTool(server.id, tool.name, { paid: !tool.paid }))
                                                }
                                            />
                                            <button
                                                class="btn btn-quiet btn-sm"
                                                aria-expanded={trying === tool.name}
                                                onClick={() => {
                                                    setTrying(trying === tool.name ? null : tool.name);
                                                    setTried(null);
                                                    setArgs('{}');
                                                }}
                                            >
                                                {cap(trying === tool.name ? tr.mcp.hide : tr.mcp.tryIt)}
                                            </button>
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
                                            <button
                                                class="btn btn-plain btn-sm self-start"
                                                disabled={busy === `try:${tool.name}`}
                                                aria-busy={busy === `try:${tool.name}` || undefined}
                                                onClick={() => void tryTool(tool)}
                                            >
                                                {spin(busy === `try:${tool.name}`)}
                                                {cap(tr.mcp.call)}
                                            </button>
                                            {tried?.tool === tool.name && (
                                                <p class={`t-mono m-0 ${tried.isError ? 'c-dot-offline' : 'c-text'}`} role="status">
                                                    {tr.mcp.triedResult(tried.isError, tried.ms, tried.content)}
                                                </p>
                                            )}
                                        </div>
                                    )}
                                </div>
                            ))}
                        </WinSection>
                    )}
                </>
            )}

            {tab === 'settings' && (
                <>
                    <label class="flex flex-col gap-6">
                        <span class="win-label">{tr.mcp.modelNote}</span>
                        <textarea
                            class="field r-16 min-h-64 resize-y"
                            placeholder={tr.mcp.modelNotePlaceholder}
                            disabled={!mayManage}
                            value={note}
                            onInput={(e) => setNote((e.target as HTMLTextAreaElement).value)}
                        />
                    </label>

                    {mayManage && (
                        <WinSection title={tr.mcp.whoUses} group>
                            {(
                                [
                                    ['voice', tr.mcp.voiceAllowed, byMode(tr.mcp.voiceAllowedSpeaker, tr.mcp.voiceAllowedMic), server.voice],
                                    ['adminOnly', tr.mcp.adminOnly, tr.mcp.adminOnlySub, server.adminOnly],
                                    ['outside', tr.mcp.outsideAnswers, tr.mcp.outsideAnswersSub, server.outside],
                                ] as const
                            ).map(([key, title, sub, on]) => (
                                <WinRow key={key} title={title} sub={sub}>
                                    <Switch on={on} label={title} disabled={busy === key} onChange={(next) => void act(key, () => api.updateMcpServer(server.id, { [key]: next }))} />
                                </WinRow>
                            ))}
                            <WinRow title={byMode(tr.mcp.isCapabilitySpeaker, tr.mcp.isCapabilityOwn)} sub={tr.mcp.capabilitySub}>
                                <select
                                    class="field"
                                    aria-label={byMode(tr.mcp.isCapabilitySpeaker, tr.mcp.isCapabilityOwn)}
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
                            </WinRow>
                        </WinSection>
                    )}

                    {mayManage && server.secrets.length > 0 && (
                        <WinSection title={tr.mcp.secretsTitle} group>
                            {server.secrets.map((secret) => {
                                const key = `${secret.kind}:${secret.name}`;
                                const draft = secretDraft[key] ?? '';
                                return (
                                    <form
                                        key={key}
                                        class="win-row is-col"
                                        onSubmit={(e) => {
                                            e.preventDefault();
                                            void act(key, () =>
                                                api.updateMcpServer(
                                                    server.id,
                                                    secret.kind === 'token'
                                                        ? { token: draft }
                                                        : secret.kind === 'env'
                                                          ? { env: { [secret.name]: draft } }
                                                          : { headers: { [secret.name]: draft } },
                                                ),
                                            );
                                        }}
                                    >
                                        <div class="win-row-main">
                                            <span class="win-row-t">{secret.kind === 'token' ? tr.mcp.fieldToken : <span class="win-code">{secret.name}</span>}</span>
                                            <span class="win-row-s">{secret.set ? tr.mcp.secretSet(secret.hint ?? '') : tr.mcp.secretNotSet}</span>
                                        </div>
                                        <div class="flex gap-8 flex-wrap items-center">
                                            <input
                                                class="field flex-1-1-160 min-w-0"
                                                type="password"
                                                autocomplete="off"
                                                aria-label={tr.mcp.newValueLabel(secret.name)}
                                                placeholder={tr.mcp.newValue}
                                                value={draft}
                                                onInput={(e) => setSecretDraft((all) => ({ ...all, [key]: (e.target as HTMLInputElement).value }))}
                                            />
                                            <button type="submit" class="btn btn-plain btn-sm" disabled={!draft.trim() || busy === key} aria-busy={busy === key || undefined}>
                                                {spin(busy === key)}
                                                {cap(tr.mcp.replace)}
                                            </button>
                                        </div>
                                    </form>
                                );
                            })}
                        </WinSection>
                    )}

                    {mayManage && (
                        <WinSection title={tr.ui.danger} group>
                            {server.enabled && (
                                <WinRow title={cap(tr.mcp.disable)}>
                                    <button
                                        class="btn btn-danger btn-sm"
                                        disabled={busy !== null}
                                        aria-busy={busy === 'enabled' || undefined}
                                        onClick={() => void act('enabled', () => api.updateMcpServer(server.id, { enabled: false }))}
                                    >
                                        {spin(busy === 'enabled')}
                                        {cap(tr.mcp.disable)}
                                    </button>
                                </WinRow>
                            )}
                            <WinRow title={cap(tr.mcp.remove)} sub={tr.mcp.removeText}>
                                <button class="btn btn-danger btn-sm" disabled={busy !== null} aria-busy={busy === 'remove' || undefined} onClick={() => void remove()}>
                                    {spin(busy === 'remove')}
                                    {cap(tr.mcp.remove)}
                                </button>
                            </WinRow>
                        </WinSection>
                    )}
                </>
            )}
        </Sheet>
    );
}
