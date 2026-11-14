import { useEffect, useState } from 'preact/hooks';

import { BrainPick, BrainView, LlmEffort, ProviderCheck, ProviderPreset, ProviderView, api, can, problemOf } from '../api';
import { IconKey } from '../icons';
import { BRANDS, type BrandId } from '../brand-icons';
import { BrandMark } from '../ui/brand';
import { Action, Card, CardNote } from '../ui/cards';
import { Sheet } from '../ui/sheet';
import { useSignIn } from '../ui/sign-in';
import { AskSheet } from './ask';
import { ask } from '../ui/confirm';
import { byMode } from '../words';
import { formatNumber, tr } from '../i18n';

/** Глубина мышления Claude: три понятных ступени из пяти уровней effort. */
const efforts = (): Array<{ id: LlmEffort; name: string; sub: string }> => [
    { id: 'low', name: tr.brain.effortLowName, sub: tr.brain.effortLowSub },
    { id: 'medium', name: tr.brain.effortMediumName, sub: tr.brain.effortMediumSub },
    { id: 'high', name: tr.brain.effortHighName, sub: tr.brain.effortHighSub },
];

/** Значок провайдера в круге строки (как у маршрутов): знак сервиса или ключ у своего. */
export function ProviderIcon({ brand, size = 38 }: { brand?: string; size?: number }) {
    return (
        <span class="route-icon" aria-hidden="true" style={{ width: size, height: size, flex: 'none' }}>
            {brand && brand in BRANDS ? <BrandMark id={brand as BrandId} size={Math.round(size / 2)} /> : <IconKey size={Math.round(size / 2)} />}
        </span>
    );
}

/** Итог проверки словами для карточки. */
export function checkWords(check: ProviderCheck | null): { text: string; ok: boolean | null } {
    const t = tr.brain;
    if (!check) return { text: t.notChecked, ok: null };
    if (!check.ok) return { text: check.problem ?? t.notWorking, ok: false };
    const seconds =
        check.firstWordMs !== undefined ? t.firstWord(formatNumber(check.firstWordMs / 1000, { minimumFractionDigits: 1, maximumFractionDigits: 1 })) : '';
    return { text: t.working(seconds, !!check.tools), ok: true };
}

/** Модели провайдера для выбора: что он назвал, иначе проверенная и советы пресета. */
function modelsOf(provider: ProviderView, presets: ProviderPreset[]): string[] {
    const preset = presets.find((entry) => entry.id === provider.preset);
    const fromPreset = preset?.suggested ? [preset.suggested.fast, preset.suggested.smart, ...(preset.suggested.more ?? [])] : [];
    const listed = provider.models.length ? provider.models : fromPreset;
    return [...new Set([...listed, ...(provider.check?.model ? [provider.check.model] : [])].filter(Boolean))];
}

/** Адрес в домашней сети или на этом Mac: вопросы не уходят из дома (как isLocalUrl шлюза). */
function isHomeAddress(url: string): boolean {
    try {
        const host = new URL(url).hostname.replace(/^\[|\]$/g, '');
        if (host === 'localhost' || host === '::1' || /\.(local|lan|home\.arpa)$/.test(host)) return true;
        const [a, b] = host.split('.').map(Number);
        return a === 127 || a === 10 || (a === 192 && b === 168) || (a === 172 && b >= 16 && b <= 31);
    } catch {
        return false;
    }
}

const pickValue = (pick: BrainPick | null): string => (pick ? `${pick.provider}\u0000${pick.model}` : '');
const parsePick = (value: string): BrainPick | null => {
    if (!value) return null;
    const [provider, model] = value.split('\u0000');
    return { provider, model };
};

/**
 * «Мозг»: кто думает над ответами. Сверху — быстрая модель (разговор) и умная (сказки, игры, сложное) из любых
 * добавленных провайдеров, ниже — сами провайдеры со своими ключами. Ключи пульт видит только хвостом.
 */
export function BrainTab() {
    const [view, setView] = useState<BrainView | null>(null);
    const [loadError, setLoadError] = useState<string | null>(null);
    const [notes, setNotes] = useState<Record<string, CardNote>>({});
    const [busy, setBusy] = useState<Record<string, boolean>>({});
    const [adding, setAdding] = useState(false);
    const [keyFor, setKeyFor] = useState<string | null>(null);
    const [keyDraft, setKeyDraft] = useState('');
    const [typed, setTyped] = useState<Record<string, string>>({});
    const [asking, setAsking] = useState(false);
    const mayManage = can('connectors.manage');
    const mayChoose = can('models.manage');

    const load = () =>
        api
            .brain()
            .then((fresh) => {
                setView(fresh);
                setLoadError(null);
            })
            .catch((e) => setLoadError(problemOf(e)));

    useEffect(() => {
        void load();
        const id = setInterval(() => void load(), 15_000);
        return () => clearInterval(id);
    }, []);

    const say = (id: string, note: CardNote) => setNotes((all) => ({ ...all, [id]: note }));
    const mark = (id: string, on: boolean) => setBusy((all) => ({ ...all, [id]: on }));

    const choose = async (profile: 'fast' | 'smart' | 'backup', pick: BrainPick | null) => {
        mark(profile, true);
        say('choice', {});
        try {
            setView(await api.chooseBrain({ [profile]: pick }));
            say('choice', { notice: tr.brain.applied });
        } catch (e) {
            say('choice', { error: problemOf(e) });
        } finally {
            mark(profile, false);
        }
    };

    const setEffort = async (effort: LlmEffort) => {
        mark('effort', true);
        try {
            setView(await api.chooseBrain({ effort }));
            say('choice', { notice: tr.brain.applied });
        } catch (e) {
            say('choice', { error: problemOf(e) });
        } finally {
            mark('effort', false);
        }
    };

    const check = async (id: string, done?: string) => {
        mark(id, true);
        say(id, done ? { notice: done } : {});
        try {
            const result = await api.checkProvider(id);
            const words = checkWords(result);
            say(id, words.ok ? { notice: tr.brain.checkedNote(done ?? '', words.text) } : { error: words.text });
            await load();
        } catch (e) {
            say(id, { error: problemOf(e) });
        } finally {
            mark(id, false);
        }
    };

    /** «Войти через …» на карточке: ключ придёт входом, потом провайдер сам проверится. */
    const signIn = useSignIn((id, title) => void check(id, tr.brain.signedIn(title)));

    const saveKey = async (provider: ProviderView, key: string) => {
        mark(provider.id, true);
        try {
            await api.updateProvider(provider.id, { key });
            setKeyFor(null);
            setKeyDraft('');
            say(provider.id, { notice: key ? tr.brain.keySaved : tr.brain.keyRemoved });
            await load();
        } catch (e) {
            say(provider.id, { error: problemOf(e) });
        } finally {
            mark(provider.id, false);
        }
    };

    const remove = async (provider: ProviderView) => {
        if (!(await ask({ title: tr.brain.removeTitle(provider.title), text: tr.brain.removeText, ok: tr.brain.removeOk, danger: true }))) return;
        mark(provider.id, true);
        try {
            await api.removeProvider(provider.id);
            await load();
        } catch (e) {
            say(provider.id, { error: problemOf(e) });
        } finally {
            mark(provider.id, false);
        }
    };

    if (!view) {
        return (
            <section class="card glass">
                <p class="t-meta m-0" role={loadError ? 'alert' : undefined}>
                    {loadError ?? tr.brain.askingGateway}
                </p>
            </section>
        );
    }

    const { providers, presets, choice, status, env } = view;
    const anthropic = providers.some((provider) => provider.kind === 'anthropic' && [choice.fast, choice.smart].some((pick) => pick?.provider === provider.id));
    /** Отвечает подписка (ChatGPT): рядом с выбором модели — что тратится и где лимиты, как просит OpenAI. */
    const plan = [choice.fast, choice.smart]
        .map((pick) => providers.find((provider) => provider.id === pick?.provider && provider.account))
        .map((provider) => provider && presets.find((preset) => preset.id === provider.preset))
        .find((preset) => preset?.signIn?.keyless);
    const envAnswers = status.source !== 'panel';
    const backup = choice.backup ?? null;
    const envLabel = tr.brain.envLabel(env.title, env.models.fast);
    /** Куда уйдёт вопрос, если ответит запасная: её название и дома она или в интернете. */
    const backupWhere = !backup
        ? null
        : backup.provider === 'env'
          ? { title: env.title, cloud: env.cloud }
          : providers.find((provider) => provider.id === backup.provider) ?? null;
    const chosenOf = (profile: 'fast' | 'smart' | 'backup') => (profile === 'fast' ? choice.fast : profile === 'smart' ? choice.smart : backup);
    const options = (profile: 'fast' | 'smart' | 'backup') => (
        <>
            <option value="">
                {profile === 'backup' ? tr.brain.optNotNeeded : profile === 'smart' ? tr.brain.optLikeFast : env.usable ? envLabel : tr.brain.optNotChosen}
            </option>
            {profile === 'backup' && env.usable && (
                <option value={pickValue({ provider: 'env', model: backup?.provider === 'env' ? backup.model : env.models.fast })}>{envLabel}</option>
            )}
            {providers.map((provider) => {
                const models = modelsOf(provider, presets);
                const chosen = chosenOf(profile);
                const extra = chosen?.provider === provider.id && !models.includes(chosen.model) ? [chosen.model] : [];
                return (
                    <optgroup key={provider.id} label={provider.title}>
                        {[...extra, ...models].map((model) => (
                            <option key={model} value={pickValue({ provider: provider.id, model })}>
                                {model}
                            </option>
                        ))}
                    </optgroup>
                );
            })}
        </>
    );

    return (
        <>
            <section class="card glass card-list" aria-label={tr.brain.whoThinks}>
                <div class="section-head flex items-center justify-between min-h-44 gap-8 flex-wrap">
                    <h2 class="t-title-xs m-0">
                        {tr.brain.whoThinks}
                    </h2>
                    <span class="flex items-center gap-8">
                        <span class="t-meta">{!envAnswers ? tr.brain.fromNextQuestion : env.usable ? tr.brain.nowEnvModel : tr.brain.noModelChosen}</span>
                        {can('mcp.use') && (status.source === 'panel' || env.usable) && <Action label={tr.brain.ask} onClick={() => setAsking(true)} />}
                    </span>
                </div>
                {(['fast', 'smart'] as const).map((profile) => (
                    <label
                        key={profile}
                        class="list-row"
                    >
                        <span class="flex-1 min-w-180 flex flex-col gap-2">
                            <b>{profile === 'fast' ? tr.brain.fast : tr.brain.smart}</b>
                            <span class="t-caption">{profile === 'fast' ? tr.brain.fastSub : tr.brain.smartSub}</span>
                        </span>
                        <select
                            class="field max-w-full min-w-0 flex-1-1-220"
                            value={pickValue(profile === 'fast' ? choice.fast : choice.smart)}
                            disabled={!mayChoose || busy[profile] || (profile === 'smart' && !choice.fast)}
                            onChange={(e) => void choose(profile, parsePick((e.target as HTMLSelectElement).value))}
                        >
                            {options(profile)}
                        </select>
                    </label>
                ))}
                <label class="list-row">
                    <span class="flex-1 min-w-180 flex flex-col gap-2">
                        <b>{tr.brain.backup}</b>
                        <span class="t-caption">{tr.brain.backupSub}</span>
                    </span>
                    <select
                        class="field max-w-full min-w-0 flex-1-1-220"
                        value={pickValue(backup)}
                        disabled={!mayChoose || busy.backup}
                        onChange={(e) => void choose('backup', parsePick((e.target as HTMLSelectElement).value))}
                    >
                        {options('backup')}
                    </select>
                </label>
                {backupWhere && (
                    <p class="t-caption m-0 mt-4">
                        {tr.brain.backupExplain(backupWhere.title, !!backupWhere.cloud)}
                    </p>
                )}
                {anthropic && (
                    <div class="list-row">
                        <span class="flex-1 min-w-180 flex flex-col gap-2">
                            <b>{tr.brain.depth}</b>
                            <span class="t-caption">{tr.brain.depthSub}</span>
                        </span>
                        <div role="radiogroup" aria-label={tr.brain.depth} class="flex gap-6 flex-wrap">
                            {efforts().map((effort) => (
                                <button
                                    key={effort.id}
                                    role="radio"
                                    aria-checked={choice.effort === effort.id}
                                    title={effort.sub}
                                    class={`pill ${choice.effort === effort.id ? 'pill-own' : 'pill-off'} cursor-pointer py-8 px-14 min-h-36`}
                                    disabled={!mayChoose || busy.effort}
                                    onClick={() => void setEffort(effort.id)}
                                >
                                    {effort.name}
                                </button>
                            ))}
                        </div>
                    </div>
                )}
                {plan?.signIn && (
                    <p class="t-caption m-0 mt-4">
                        {tr.brain.planAnswers(plan.signIn.with)}
                        {plan.usageUrl && (
                            <>
                                {' '}
                                <a class="c-accent-text" href={plan.usageUrl} target="_blank" rel="noopener noreferrer">
                                    {tr.brain.limits}
                                </a>
                            </>
                        )}
                    </p>
                )}
                {notes.choice?.notice && (
                    <p class="t-caption m-0 mt-4 c-accent-text" role="status">
                        {notes.choice.notice}
                    </p>
                )}
                {notes.choice?.error && (
                    <p class="t-caption m-0 mt-4 c-dot-offline" role="alert">
                        {notes.choice.error}
                    </p>
                )}
                {!mayChoose && <p class="t-caption m-0 mt-8">{tr.brain.adminChooses}</p>}
            </section>

            <section class="card glass card-list" aria-label={tr.brain.providersLabel}>
                <div class="flex items-center justify-between min-h-44 gap-8">
                    <h2 class="t-title-xs m-0">
                        {tr.brain.providers}
                    </h2>
                    {mayManage && <Action label={tr.brain.add} tone="on" onClick={() => setAdding(true)} />}
                </div>
                {providers.length === 0 && (
                    <p class="t-caption m-0 mb-8">
                        {tr.brain.noProviders}
                    </p>
                )}
                {providers.map((provider) => {
                    const words = checkWords(provider.check);
                    const answering = [choice.fast, choice.smart].some((pick) => pick?.provider === provider.id);
                    const models = modelsOf(provider, presets);
                    const preset = presets.find((entry) => entry.id === provider.preset);
                    const via = preset?.signIn;
                    const account = provider.account;
                    const keyLine = account
                        ? account.signedIn
                            ? tr.brain.keyPlan(via?.with ?? '', account.who ?? '')
                            : tr.brain.needLogin
                        : provider.key.set
                          ? tr.brain.keyHint(provider.key.hint ?? '')
                          : provider.needsKey
                            ? tr.brain.noKey
                            : tr.brain.keyless;
                    const noKey = via ? tr.brain.noKeyVia(via.with) : tr.brain.noKeyPlain;
                    const accountNote: CardNote | undefined = !account
                        ? undefined
                        : !account.signedIn
                          ? { warning: tr.brain.noLogin(via?.with ?? tr.brain.noLoginService) }
                          : account.problem === 'limit'
                            ? { warning: tr.brain.planLimit(via?.with ?? '') }
                            : undefined;
                    return (
                        <Card
                            key={provider.id}
                            icon={<ProviderIcon brand={provider.brand} />}
                            title={provider.title}
                            pills={[
                                provider.cloud ? { text: tr.brain.pillCloud, cls: 'pill-yandex' } : { text: tr.brain.pillHome, cls: 'pill-own' },
                                ...(answering ? [{ text: tr.brain.pillAnswers, cls: 'pill-own' }] : []),
                                ...(backup?.provider === provider.id ? [{ text: tr.brain.pillBackup, cls: 'pill-off' }] : []),
                            ]}
                            description={`${words.text} · ${keyLine}`}
                            note={
                                notes[provider.id] ??
                                accountNote ??
                                (provider.needsKey && !provider.key.set ? { warning: noKey } : words.ok === false ? { error: words.text } : undefined)
                            }
                            action={null}
                        >
                            {mayManage && (
                                <div class="flex items-center gap-6 flex-wrap pl-50">
                                    <Action label={tr.brain.check} tone="on" busy={busy[provider.id]} onClick={() => void check(provider.id)} />
                                    {via && (
                                        <Action
                                            label={account?.signedIn ? tr.brain.signInAgain : tr.brain.signInAction}
                                            tone={(account ? account.signedIn : provider.key.set) ? 'off' : 'on'}
                                            busy={signIn.starting && signIn.target === provider.id}
                                            disabled={busy[provider.id]}
                                            onClick={() => signIn.start(provider.id, via.with, () => api.signInProvider(provider.id))}
                                        />
                                    )}
                                    {!via?.keyless && (
                                        <Action
                                            label={tr.brain.key}
                                            disabled={busy[provider.id]}
                                            onClick={() => {
                                                setKeyFor(keyFor === provider.id ? null : provider.id);
                                                setKeyDraft('');
                                            }}
                                        />
                                    )}
                                    {preset?.usageUrl && (
                                        <a
                                            class="pill pill-off py-8 px-14 min-h-36 inline-flex items-center"
                                            href={preset.usageUrl}
                                            target="_blank"
                                            rel="noopener noreferrer"
                                        >
                                            {tr.brain.limitsLower}
                                        </a>
                                    )}
                                    <Action label={tr.brain.remove} disabled={busy[provider.id]} onClick={() => void remove(provider)} />
                                </div>
                            )}
                            {signIn.target === provider.id && signIn.steps && <div class="pl-50">{signIn.steps}</div>}
                            {keyFor === provider.id && (
                                <form
                                    class="flex gap-8 flex-wrap pl-50"
                                    onSubmit={(e) => {
                                        e.preventDefault();
                                        void saveKey(provider, keyDraft.trim());
                                    }}
                                >
                                    <input
                                        class="field flex-1-1-200 min-w-0"
                                        type="password"
                                        autocomplete="off"
                                        aria-label={tr.brain.newKeyLabel(provider.title)}
                                        placeholder={tr.brain.newKey}
                                        value={keyDraft}
                                        onInput={(e) => setKeyDraft((e.target as HTMLInputElement).value)}
                                    />
                                    <Action label={tr.brain.save} tone="on" busy={busy[provider.id]} disabled={!keyDraft.trim()} onClick={() => void saveKey(provider, keyDraft.trim())} />
                                    {provider.key.set && <Action label={tr.brain.removeKey} onClick={() => void saveKey(provider, '')} />}
                                </form>
                            )}
                            {mayChoose && models.length === 0 && (
                                <form
                                    class="flex gap-8 flex-wrap pl-50"
                                    onSubmit={(e) => {
                                        e.preventDefault();
                                        const model = (typed[provider.id] ?? '').trim();
                                        if (model) void choose('fast', { provider: provider.id, model });
                                    }}
                                >
                                    <input
                                        class="field flex-1-1-200 min-w-0"
                                        aria-label={tr.brain.modelLabel(provider.title)}
                                        placeholder={tr.brain.modelPlaceholder}
                                        value={typed[provider.id] ?? ''}
                                        onInput={(e) => setTyped((all) => ({ ...all, [provider.id]: (e.target as HTMLInputElement).value }))}
                                    />
                                    <Action
                                        label={tr.brain.makeFast}
                                        disabled={!(typed[provider.id] ?? '').trim()}
                                        onClick={() => void choose('fast', { provider: provider.id, model: (typed[provider.id] ?? '').trim() })}
                                    />
                                </form>
                            )}
                        </Card>
                    );
                })}
                <Card
                    icon={<ProviderIcon />}
                    title={tr.brain.envTitle}
                    pills={envAnswers && env.usable ? [{ text: tr.brain.pillAnswers, cls: 'pill-own' }] : []}
                    description={
                        env.usable
                            ? env.models.smart !== env.models.fast
                                ? tr.brain.envSmart(env.title, env.models.fast, env.models.smart)
                                : tr.brain.envPlain(env.title, env.models.fast)
                            : tr.brain.envUnset
                    }
                    meta={backup?.provider === 'env' ? tr.brain.envBackup : undefined}
                    action={null}
                />
            </section>

            {asking && <AskSheet onClose={() => setAsking(false)} />}
            {adding && (
                <AddProvider
                    presets={presets}
                    firstOne={!choice.fast}
                    onClose={() => setAdding(false)}
                    onDone={(fresh) => {
                        if (fresh) setView(fresh);
                        else void load();
                    }}
                />
            )}
        </>
    );
}

/**
 * Шит «Добавить провайдера»: сначала какой (готовые и «свой»), потом ключ, адрес и поля пресета. «Проверить и
 * сохранить» сохраняет и сразу проверяет; если это первый провайдер и он работает — предложит отвечать им. У сервиса со
 * входом (OpenRouter) ключ можно не вставлять: «Войти через …» добавляет провайдера без ключа, ключ приходит входом.
 */
function AddProvider({
    presets,
    firstOne,
    onClose,
    onDone,
}: {
    presets: ProviderPreset[];
    firstOne: boolean;
    onClose: () => void;
    onDone: (fresh?: BrainView) => void;
}) {
    const [preset, setPreset] = useState<ProviderPreset | null>(null);
    const [key, setKey] = useState('');
    const [url, setUrl] = useState('');
    const [title, setTitle] = useState('');
    const [fields, setFields] = useState<Record<string, string>>({});
    const [busy, setBusy] = useState(false);
    const [error, setError] = useState<string | null>(null);
    const [result, setResult] = useState<{ provider: ProviderView; check: ProviderCheck } | null>(null);
    /** Провайдер, уже добавленный входом: ключ, вставленный потом, ложится на него, а не на второго такого же. */
    const [created, setCreated] = useState<ProviderView | null>(null);

    const checked = async (provider: ProviderView) => {
        setBusy(true);
        setError(null);
        try {
            setResult({ provider, check: await api.checkProvider(provider.id) });
            onDone();
        } catch (e) {
            setError(problemOf(e));
        } finally {
            setBusy(false);
        }
    };

    const signIn = useSignIn(() => {
        if (created) void checked(created);
    });

    const start = (chosen: ProviderPreset) => {
        setPreset(chosen);
        setUrl(chosen.baseUrl ?? '');
        setTitle('');
        setFields({});
        setKey('');
        setError(null);
        if (created && created.preset !== chosen.id) setCreated(null);
    };

    const fieldsOf = (chosen: ProviderPreset) => ({
        ...(title.trim() ? { title: title.trim() } : {}),
        ...(chosen.editableUrl ? { baseUrl: url.trim() } : {}),
        options: fields,
    });

    /** Войти: провайдер добавляется без ключа (если его ещё нет), потом страница сервиса. */
    const beginSignIn = (chosen: ProviderPreset, service: string) =>
        signIn.start('new', service, async () => {
            const provider = created ?? (await api.addProvider({ preset: chosen.id, ...fieldsOf(chosen) }));
            setCreated(provider);
            onDone();
            return api.signInProvider(provider.id);
        });

    const save = async () => {
        if (!preset) return;
        setBusy(true);
        setError(null);
        try {
            const given = { ...fieldsOf(preset), ...(key.trim() ? { key: key.trim() } : {}) };
            const provider = created ? await api.updateProvider(created.id, given) : await api.addProvider({ preset: preset.id, ...given });
            const check = await api.checkProvider(provider.id);
            setResult({ provider, check });
            onDone();
        } catch (e) {
            setError(problemOf(e));
        } finally {
            setBusy(false);
        }
    };

    const useIt = async () => {
        if (!result?.check.model) return;
        setBusy(true);
        try {
            onDone(await api.chooseBrain({ fast: { provider: result.provider.id, model: result.check.model } }));
            onClose();
        } catch (e) {
            setError(problemOf(e));
        } finally {
            setBusy(false);
        }
    };

    if (!preset) {
        return (
            <Sheet title={tr.brain.addTitle} about={tr.brain.addAbout} onClose={onClose}>
                <div class="grid grid-fill-220 gap-8">
                    {presets.map((entry) => (
                        <button
                            key={entry.id}
                            class="dash-option flex items-center gap-12 text-left border-glass r-16 py-10 px-12 cursor-pointer"
                            onClick={() => start(entry)}
                        >
                            <ProviderIcon brand={entry.brand} />
                            <span class="flex flex-col gap-2 min-w-0">
                                <b class="flex gap-6 items-center flex-wrap">
                                    {entry.title}
                                    {entry.local && <span class="pill pill-own">{tr.brain.pillHome}</span>}
                                </b>
                                <span class="t-caption">{entry.description}</span>
                            </span>
                        </button>
                    ))}
                </div>
            </Sheet>
        );
    }

    const words = result ? checkWords(result.check) : null;
    return (
        <Sheet
            title={preset.title}
            about={
                preset.local || (preset.editableUrl && url.trim() && isHomeAddress(url.trim()))
                    ? tr.brain.aboutHome
                    : preset.editableUrl && !url.trim()
                      ? tr.brain.aboutOwnAddress
                      : tr.brain.aboutInternet
            }
            icon={<ProviderIcon brand={preset.brand} />}
            onClose={onClose}
            footer={
                result ? (
                    <>
                        {firstOne && result.check.ok && result.check.model && (
                            <button class="pill pill-own dash-confirm-btn self-start" disabled={busy} onClick={() => void useIt()}>
                                {tr.brain.useIt(result.check.model)}
                            </button>
                        )}
                        <button class="dash-done" onClick={onClose}>
                            {tr.brain.done}
                        </button>
                    </>
                ) : (
                    <div class="flex gap-8 flex-wrap">
                        {!preset.signIn?.keyless && (
                            <button
                                class="pill pill-own dash-confirm-btn"
                                disabled={busy || (preset.needsKey && !key.trim()) || (preset.editableUrl && !url.trim())}
                                onClick={() => void save()}
                            >
                                {busy ? tr.brain.checking : tr.brain.checkAndSave}
                            </button>
                        )}
                        <button class="pill pill-off dash-confirm-btn" onClick={() => setPreset(null)}>
                            {tr.brain.back}
                        </button>
                    </div>
                )
            }
        >
            {!result && (
                <form
                    class="flex flex-col gap-10"
                    onSubmit={(e) => {
                        e.preventDefault();
                        void save();
                    }}
                >
                    <p class="t-caption m-0">
                        {preset.description}
                    </p>
                    {preset.signIn && (
                        <div class="flex flex-col gap-6">
                            <button
                                type="button"
                                class="pill pill-own dash-confirm-btn self-start"
                                disabled={busy || signIn.starting}
                                onClick={() => preset.signIn && beginSignIn(preset, preset.signIn.with)}
                            >
                                {signIn.starting ? tr.brain.opening : (preset.signIn.label ?? tr.brain.signInWith(preset.signIn.with))}
                            </button>
                            {preset.signIn.keyless ? (
                                <span class="t-caption">
                                    {tr.brain.keylessNote(preset.signIn.with)}
                                    {preset.usageUrl && (
                                        <>
                                            {' '}
                                            <a class="c-accent-text" href={preset.usageUrl} target="_blank" rel="noopener noreferrer">
                                                {tr.brain.limits}
                                            </a>
                                        </>
                                    )}
                                </span>
                            ) : (
                                <span class="t-caption">
                                    {tr.brain.keyCreated(preset.signIn.with)}
                                </span>
                            )}
                            {signIn.steps}
                        </div>
                    )}
                    {preset.editableUrl && (
                        <label class="flex flex-col gap-4">
                            <span class="t-caption">{tr.brain.address}</span>
                            <input class="field" value={url} placeholder="http://localhost:8000/v1" onInput={(e) => setUrl((e.target as HTMLInputElement).value)} />
                        </label>
                    )}
                    {!preset.signIn?.keyless && (
                        <label class="flex flex-col gap-4">
                            <span class="t-caption">{preset.needsKey ? tr.brain.keyField : tr.brain.keyFieldOptional}</span>
                            <input
                                class="field"
                                type="password"
                                autocomplete="off"
                                value={key}
                                onInput={(e) => setKey((e.target as HTMLInputElement).value)}
                            />
                        </label>
                    )}
                    {preset.keyUrl && (
                        <a class="t-caption c-accent-text" href={preset.keyUrl} target="_blank" rel="noopener noreferrer">
                            {tr.brain.whereKey}
                        </a>
                    )}
                    {(preset.fields ?? []).map((field) => (
                        <label key={field.id} class="flex flex-col gap-4">
                            <span class="t-caption">{field.title}</span>
                            <input
                                class="field"
                                value={fields[field.id] ?? ''}
                                placeholder={field.placeholder}
                                onInput={(e) => setFields((all) => ({ ...all, [field.id]: (e.target as HTMLInputElement).value }))}
                            />
                        </label>
                    ))}
                    <label class="flex flex-col gap-4">
                        <span class="t-caption">{tr.brain.titleField}</span>
                        <input class="field" value={title} placeholder={preset.title} onInput={(e) => setTitle((e.target as HTMLInputElement).value)} />
                    </label>
                    <p class="t-caption m-0">
                        {preset.signIn?.keyless
                            ? byMode(tr.brain.loginStoredSpeaker, tr.brain.loginStoredComputer)
                            : tr.brain.keyStored}
                    </p>
                </form>
            )}
            {result && words && (
                <p role="status" class={`t-meta m-0 ${words.ok ? 'c-accent-text' : 'c-dot-offline'}`}>
                    {words.ok ? tr.brain.savedOk(words.text) : tr.brain.savedFailed(words.text)}
                </p>
            )}
            {error && (
                <p role="alert" class="t-meta m-0 c-dot-offline">
                    {error}
                </p>
            )}
        </Sheet>
    );
}
