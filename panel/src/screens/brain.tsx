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

/** Глубина мышления Claude: три понятных ступени из пяти уровней effort. */
const EFFORTS: Array<{ id: LlmEffort; name: string; sub: string }> = [
    { id: 'low', name: 'Быстро', sub: 'думает меньше всех, первое слово за секунду' },
    { id: 'medium', name: 'С умом', sub: 'думает чуть дольше, точнее' },
    { id: 'high', name: 'Глубоко', sub: 'думает дольше всех над сложным' },
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
    if (!check) return { text: 'ещё не проверен', ok: null };
    if (!check.ok) return { text: check.problem ?? 'не работает', ok: false };
    const seconds = check.firstWordMs !== undefined ? ` · первое слово за ${(check.firstWordMs / 1000).toFixed(1).replace('.', ',')} с` : '';
    return { text: `работает${seconds} · ${check.tools ? 'умеет инструменты' : 'без инструментов'}`, ok: true };
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
            say('choice', { notice: 'Применено — со следующего вопроса.' });
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
            say('choice', { notice: 'Применено — со следующего вопроса.' });
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
            say(id, words.ok ? { notice: `${done ? `${done} ` : ''}Проверено: ${words.text}.` } : { error: words.text });
            await load();
        } catch (e) {
            say(id, { error: problemOf(e) });
        } finally {
            mark(id, false);
        }
    };

    /** «Войти через …» на карточке: ключ придёт входом, потом провайдер сам проверится. */
    const signIn = useSignIn((id, title) => void check(id, `Вход в «${title}» выполнен — ключ у шлюза.`));

    const saveKey = async (provider: ProviderView, key: string) => {
        mark(provider.id, true);
        try {
            await api.updateProvider(provider.id, { key });
            setKeyFor(null);
            setKeyDraft('');
            say(provider.id, { notice: key ? 'Ключ сохранён. Нажмите «проверить».' : 'Ключ убран.' });
            await load();
        } catch (e) {
            say(provider.id, { error: problemOf(e) });
        } finally {
            mark(provider.id, false);
        }
    };

    const remove = async (provider: ProviderView) => {
        if (!(await ask({ title: `Убрать «${provider.title}»?`, text: 'Вместе с ключом.', ok: 'Убрать', danger: true }))) return;
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
                    {loadError ?? 'Спрашиваем шлюз…'}
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
    const envLabel = `из .env: ${env.title} · ${env.models.fast}`;
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
                {profile === 'backup' ? 'не нужна' : profile === 'smart' ? 'как быстрая' : env.usable ? envLabel : 'не выбрана'}
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
            <section class="card glass card-list" aria-label="Кто думает">
                <div class="section-head flex items-center justify-between min-h-44 gap-8 flex-wrap">
                    <h2 class="t-title-xs m-0">
                        Кто думает
                    </h2>
                    <span class="flex items-center gap-8">
                        <span class="t-meta">{!envAnswers ? 'со следующего вопроса' : env.usable ? 'сейчас — модель из .env' : 'модель не выбрана'}</span>
                        {can('mcp.use') && (status.source === 'panel' || env.usable) && <Action label="спросить" onClick={() => setAsking(true)} />}
                    </span>
                </div>
                {(['fast', 'smart'] as const).map((profile) => (
                    <label
                        key={profile}
                        class="list-row"
                    >
                        <span class="flex-1 min-w-180 flex flex-col gap-2">
                            <b>{profile === 'fast' ? 'Быстрая' : 'Умная'}</b>
                            <span class="t-caption">{profile === 'fast' ? 'разговор и команды' : 'сказки, игры и сложные вопросы'}</span>
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
                        <b>Запасная</b>
                        <span class="t-caption">если выбранная не ответит</span>
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
                        Запасная отвечает, когда выбранная модель не ответила: кончился лимит подписки, нет связи или сервис отказал. Вопрос
                        тогда уйдёт в «{backupWhere.title}» — {backupWhere.cloud ? 'в интернет' : 'дома, без интернета'}.
                    </p>
                )}
                {anthropic && (
                    <div class="list-row">
                        <span class="flex-1 min-w-180 flex flex-col gap-2">
                            <b>Глубина мышления</b>
                            <span class="t-caption">только у Claude: сколько думать до первого слова</span>
                        </span>
                        <div role="radiogroup" aria-label="Глубина мышления" class="flex gap-6 flex-wrap">
                            {EFFORTS.map((effort) => (
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
                        Отвечает подписка {plan.signIn.with}: тратится ваш план, а не ключ API.
                        {plan.usageUrl && (
                            <>
                                {' '}
                                <a class="c-accent-text" href={plan.usageUrl} target="_blank" rel="noopener noreferrer">
                                    Лимиты
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
                {!mayChoose && <p class="t-caption m-0 mt-8">Выбирать модели может администратор.</p>}
            </section>

            <section class="card glass card-list" aria-label="Провайдеры моделей">
                <div class="flex items-center justify-between min-h-44 gap-8">
                    <h2 class="t-title-xs m-0">
                        Провайдеры
                    </h2>
                    {mayManage && <Action label="добавить" tone="on" onClick={() => setAdding(true)} />}
                </div>
                {providers.length === 0 && (
                    <p class="t-caption m-0 mb-8">
                        Пока ни одного: отвечает модель из .env. Добавьте провайдера со своим ключом — Claude, OpenAI, модель на этом Mac или
                        любой совместимый сервис, — или войдите в аккаунт OpenRouter.
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
                            ? `подписка ${via?.with ?? ''}${account.who ? ` · ${account.who}` : ''}`
                            : 'нужно войти'
                        : provider.key.set
                          ? `ключ ${provider.key.hint ?? ''}`
                          : provider.needsKey
                            ? 'нет ключа'
                            : 'без ключа';
                    const noKey = via ? `Без ключа сервис не ответит: войдите через ${via.with} или вставьте ключ.` : 'Без ключа сервис не ответит.';
                    const accountNote: CardNote | undefined = !account
                        ? undefined
                        : !account.signedIn
                          ? { warning: `Без входа ${via?.with ?? 'сервис'} не ответит: нажмите «войти».` }
                          : account.problem === 'limit'
                            ? { warning: `Лимит подписки ${via?.with ?? ''} исчерпан: подождите, пока он восстановится, или выберите другую модель.` }
                            : undefined;
                    return (
                        <Card
                            key={provider.id}
                            icon={<ProviderIcon brand={provider.brand} />}
                            title={provider.title}
                            pills={[
                                provider.cloud ? { text: 'облако', cls: 'pill-yandex' } : { text: 'дома', cls: 'pill-own' },
                                ...(answering ? [{ text: 'отвечает', cls: 'pill-own' }] : []),
                                ...(backup?.provider === provider.id ? [{ text: 'запасная', cls: 'pill-off' }] : []),
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
                                    <Action label="проверить" tone="on" busy={busy[provider.id]} onClick={() => void check(provider.id)} />
                                    {via && (
                                        <Action
                                            label={account?.signedIn ? 'войти заново' : 'войти'}
                                            tone={(account ? account.signedIn : provider.key.set) ? 'off' : 'on'}
                                            busy={signIn.starting && signIn.target === provider.id}
                                            disabled={busy[provider.id]}
                                            onClick={() => signIn.start(provider.id, via.with, () => api.signInProvider(provider.id))}
                                        />
                                    )}
                                    {!via?.keyless && (
                                        <Action
                                            label="ключ"
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
                                            лимиты
                                        </a>
                                    )}
                                    <Action label="убрать" disabled={busy[provider.id]} onClick={() => void remove(provider)} />
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
                                        aria-label={`Новый ключ ${provider.title}`}
                                        placeholder="Новый ключ"
                                        value={keyDraft}
                                        onInput={(e) => setKeyDraft((e.target as HTMLInputElement).value)}
                                    />
                                    <Action label="сохранить" tone="on" busy={busy[provider.id]} disabled={!keyDraft.trim()} onClick={() => void saveKey(provider, keyDraft.trim())} />
                                    {provider.key.set && <Action label="убрать ключ" onClick={() => void saveKey(provider, '')} />}
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
                                        aria-label={`Модель ${provider.title}`}
                                        placeholder="Модель, например llama3.1:8b"
                                        value={typed[provider.id] ?? ''}
                                        onInput={(e) => setTyped((all) => ({ ...all, [provider.id]: (e.target as HTMLInputElement).value }))}
                                    />
                                    <Action
                                        label="сделать быстрой"
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
                    title="Из .env"
                    pills={envAnswers && env.usable ? [{ text: 'отвечает', cls: 'pill-own' }] : []}
                    description={
                        env.usable
                            ? `${env.title} · ${env.models.fast}${env.models.smart !== env.models.fast ? `, умная — ${env.models.smart}` : ''}`
                            : 'не задана: DIALOG_PROVIDER=none'
                    }
                    meta={backup?.provider === 'env' ? 'запасная' : undefined}
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
            <Sheet title="Добавить провайдера" about="Готовые адреса или свой сервис — ключ ваш" onClose={onClose}>
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
                                    {entry.local && <span class="pill pill-own">дома</span>}
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
                    ? 'адрес дома — вопросы не уходят из дома'
                    : preset.editableUrl && !url.trim()
                      ? 'свой адрес: дома или в интернете'
                      : 'вопросы уходят в интернет'
            }
            icon={<ProviderIcon brand={preset.brand} />}
            onClose={onClose}
            footer={
                result ? (
                    <>
                        {firstOne && result.check.ok && result.check.model && (
                            <button class="pill pill-own dash-confirm-btn self-start" disabled={busy} onClick={() => void useIt()}>
                                Отвечать им: {result.check.model}
                            </button>
                        )}
                        <button class="dash-done" onClick={onClose}>
                            Готово
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
                                {busy ? 'Проверяем…' : 'Проверить и сохранить'}
                            </button>
                        )}
                        <button class="pill pill-off dash-confirm-btn" onClick={() => setPreset(null)}>
                            Назад
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
                                {signIn.starting ? 'Открываем…' : (preset.signIn.label ?? `Войти через ${preset.signIn.with}`)}
                            </button>
                            {preset.signIn.keyless ? (
                                <span class="t-caption">
                                    Ответы пойдут из вашей подписки {preset.signIn.with}, ключ API не нужен. Сколько DuoWave может тратить из
                                    плана, задаётся в настройках {preset.signIn.with}.
                                    {preset.usageUrl && (
                                        <>
                                            {' '}
                                            <a class="c-accent-text" href={preset.usageUrl} target="_blank" rel="noopener noreferrer">
                                                Лимиты
                                            </a>
                                        </>
                                    )}
                                </span>
                            ) : (
                                <span class="t-caption">
                                    Ключ создастся в вашем аккаунте {preset.signIn.with} и придёт в шлюз сам. Удалить его можно там же. Или вставьте
                                    свой ключ ниже.
                                </span>
                            )}
                            {signIn.steps}
                        </div>
                    )}
                    {preset.editableUrl && (
                        <label class="flex flex-col gap-4">
                            <span class="t-caption">Адрес</span>
                            <input class="field" value={url} placeholder="http://localhost:8000/v1" onInput={(e) => setUrl((e.target as HTMLInputElement).value)} />
                        </label>
                    )}
                    {!preset.signIn?.keyless && (
                        <label class="flex flex-col gap-4">
                            <span class="t-caption">{preset.needsKey ? 'Ключ' : 'Ключ, если сервис его просит'}</span>
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
                            Где взять ключ
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
                        <span class="t-caption">Название в пульте (необязательно)</span>
                        <input class="field" value={title} placeholder={preset.title} onInput={(e) => setTitle((e.target as HTMLInputElement).value)} />
                    </label>
                    <p class="t-caption m-0">
                        {preset.signIn?.keyless
                            ? 'Вход хранится в шлюзе зашифрованным и остаётся на этом компьютере: на колонку и другие компьютеры он не переезжает.'
                            : 'Ключ хранится в шлюзе зашифрованным; пульт потом покажет только его последние символы.'}
                    </p>
                </form>
            )}
            {result && words && (
                <p role="status" class={`t-meta m-0 ${words.ok ? 'c-accent-text' : 'c-dot-offline'}`}>
                    {words.ok ? `Сохранён. Проверка: ${words.text}.` : `Сохранён, но проверка не прошла: ${words.text}`}
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
