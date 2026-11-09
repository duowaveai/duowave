import { useEffect, useState } from 'preact/hooks';

import { BrainPick, BrainView, LlmEffort, ProviderCheck, ProviderPreset, ProviderView, api, can, problemOf } from '../api';
import { IconKey } from '../icons';
import { BRANDS, type BrandId } from '../brand-icons';
import { BrandMark } from '../ui/brand';
import { Action, Card, CardNote } from '../ui/cards';
import { Sheet } from '../ui/sheet';
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

    const choose = async (profile: 'fast' | 'smart', pick: BrainPick | null) => {
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

    const check = async (provider: ProviderView) => {
        mark(provider.id, true);
        say(provider.id, {});
        try {
            const result = await api.checkProvider(provider.id);
            const words = checkWords(result);
            say(provider.id, words.ok ? { notice: `Проверено: ${words.text}.` } : { error: words.text });
            await load();
        } catch (e) {
            say(provider.id, { error: problemOf(e) });
        } finally {
            mark(provider.id, false);
        }
    };

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
    const envAnswers = status.source !== 'panel';
    const options = (profile: 'fast' | 'smart') => (
        <>
            <option value="">
                {profile === 'smart' ? 'как быстрая' : env.usable ? `из .env: ${env.title} · ${env.models.fast}` : 'не выбрана'}
            </option>
            {providers.map((provider) => {
                const models = modelsOf(provider, presets);
                const chosen = profile === 'fast' ? choice.fast : choice.smart;
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
                        Пока ни одного: отвечает модель из .env. Добавьте провайдера со своим ключом — Claude, OpenAI, OpenRouter, модель на
                        этом Mac или любой совместимый сервис.
                    </p>
                )}
                {providers.map((provider) => {
                    const words = checkWords(provider.check);
                    const answering = [choice.fast, choice.smart].some((pick) => pick?.provider === provider.id);
                    const keyLine = provider.key.set ? `ключ ${provider.key.hint ?? ''}` : provider.needsKey ? 'нет ключа' : 'без ключа';
                    const models = modelsOf(provider, presets);
                    return (
                        <Card
                            key={provider.id}
                            icon={<ProviderIcon brand={provider.brand} />}
                            title={provider.title}
                            pills={[
                                provider.cloud ? { text: 'облако', cls: 'pill-yandex' } : { text: 'дома', cls: 'pill-own' },
                                ...(answering ? [{ text: 'отвечает', cls: 'pill-own' }] : []),
                            ]}
                            description={`${words.text} · ${keyLine}`}
                            note={
                                notes[provider.id] ??
                                (provider.needsKey && !provider.key.set ? { warning: 'Без ключа сервис не ответит.' } : words.ok === false ? { error: words.text } : undefined)
                            }
                            action={null}
                        >
                            {mayManage && (
                                <div class="flex items-center gap-6 flex-wrap pl-50">
                                    <Action label="проверить" tone="on" busy={busy[provider.id]} onClick={() => void check(provider)} />
                                    <Action
                                        label="ключ"
                                        disabled={busy[provider.id]}
                                        onClick={() => {
                                            setKeyFor(keyFor === provider.id ? null : provider.id);
                                            setKeyDraft('');
                                        }}
                                    />
                                    <Action label="убрать" disabled={busy[provider.id]} onClick={() => void remove(provider)} />
                                </div>
                            )}
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
                    meta={envAnswers ? undefined : 'запасная'}
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
 * сохранить» сохраняет и сразу проверяет; если это первый провайдер и он работает — предложит отвечать им.
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

    const start = (chosen: ProviderPreset) => {
        setPreset(chosen);
        setUrl(chosen.baseUrl ?? '');
        setTitle('');
        setFields({});
        setKey('');
        setError(null);
    };

    const save = async () => {
        if (!preset) return;
        setBusy(true);
        setError(null);
        try {
            const provider = await api.addProvider({
                preset: preset.id,
                ...(title.trim() ? { title: title.trim() } : {}),
                ...(preset.editableUrl ? { baseUrl: url.trim() } : {}),
                ...(key.trim() ? { key: key.trim() } : {}),
                options: fields,
            });
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
                        <button
                            class="pill pill-own dash-confirm-btn"
                            disabled={busy || (preset.needsKey && !key.trim()) || (preset.editableUrl && !url.trim())}
                            onClick={() => void save()}
                        >
                            {busy ? 'Проверяем…' : 'Проверить и сохранить'}
                        </button>
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
                    {preset.editableUrl && (
                        <label class="flex flex-col gap-4">
                            <span class="t-caption">Адрес</span>
                            <input class="field" value={url} placeholder="http://localhost:8000/v1" onInput={(e) => setUrl((e.target as HTMLInputElement).value)} />
                        </label>
                    )}
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
                        Ключ хранится в шлюзе зашифрованным; пульт потом покажет только его последние символы.
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
