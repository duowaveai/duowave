import { useEffect, useState } from 'preact/hooks';

import { BrainPick, BrainView, LlmEffort, ProviderCheck, ProviderPreset, ProviderView, api, can, problemOf } from '../api';
import { IconKey } from '../icons';
import { BRANDS, type BrandId } from '../brand-icons';
import { placeIcon, placeTitle } from '../helpers';
import { usePoll } from '../poll';
import { usePanel } from '../shell/context';
import { BrandMark } from '../ui/brand';
import { Action, Card, CardNote } from '../ui/cards';
import { Sheet, WinSection } from '../ui/sheet';
import { useSignIn } from '../ui/sign-in';
import { AskSheet } from './ask';
import { backupIdle, backupOf, backupPlace, backupShown, blankBackups, parsePick, pickValue } from './brain-backup';
import { ask } from '../ui/confirm';
import { byMode } from '../words';
import { formatNumber, tr } from '../i18n';
import { accessLine, accessWarning } from './brain-connect';
import { DuoWaveMark } from '../ui/duowave';

/**
 * «Что сейчас знает Мозг» (docs/plans/audio-interface.md, «Блок «Сейчас»»): тот же текст, что модель получает в каждом
 * запросе на этом компьютере, — работы, что ждёт ответа, что недавно, что на столе. Пусто — ничего не происходит.
 */
function NowKnown() {
    const [text, setText] = useState<string | null>(null);
    const [error, setError] = useState<string | null>(null);
    const read = () =>
        api
            .talkNow()
            .then((now) => {
                setText(now.text);
                setError(null);
            })
            .catch((e) => setError(problemOf(e)));
    useEffect(() => {
        void read();
        const timer = window.setInterval(() => void read(), 15_000);
        return () => window.clearInterval(timer);
    }, []);
    return (
        <section class="card glass card-list" aria-label={tr.brain.nowTitle}>
            <div class="section-head flex items-center justify-between min-h-44 gap-8 flex-wrap">
                <h2 class="t-title-xs m-0">{tr.brain.nowTitle}</h2>
                <Action label={tr.brain.nowRefresh} onClick={() => void read()} />
            </div>
            {error ? (
                <p class="t-caption m-0 c-dot-offline" role="alert">
                    {error}
                </p>
            ) : text === null ? (
                <p class="t-caption m-0">{tr.brain.nowReading}</p>
            ) : text ? (
                <pre class="t-caption m-0" style={{ whiteSpace: 'pre-wrap', fontFamily: 'inherit' }}>
                    {text}
                </pre>
            ) : (
                <p class="t-caption m-0">{tr.brain.nowEmpty}</p>
            )}
            <p class="t-caption m-0 mt-8">{tr.brain.nowAbout}</p>
        </section>
    );
}

/** Глубина мышления Claude: три понятных ступени из пяти уровней effort. */
const efforts = (): Array<{ id: LlmEffort; name: string; sub: string }> => [
    { id: 'low', name: tr.brain.effortLowName, sub: tr.brain.effortLowSub },
    { id: 'medium', name: tr.brain.effortMediumName, sub: tr.brain.effortMediumSub },
    { id: 'high', name: tr.brain.effortHighName, sub: tr.brain.effortHighSub },
];

/** Значок провайдера в круге строки (как у маршрутов): знак сервиса, ДиВи у моделей DuoWave или ключ у своего. */
export function ProviderIcon({ brand, size = 38 }: { brand?: string; size?: number }) {
    return (
        <span class="route-icon" aria-hidden="true" style={{ width: size, height: size, flex: 'none' }}>
            {brand === 'duowave' ? (
                <DuoWaveMark size={Math.round(size * 0.66)} />
            ) : brand && brand in BRANDS ? (
                <BrandMark id={brand as BrandId} size={Math.round(size / 2)} />
            ) : (
                <IconKey size={Math.round(size / 2)} />
            )}
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

/**
 * «Мозг»: кто думает над ответами. Сверху — быстрая модель (разговор) и умная (сказки, игры, сложное) из любых
 * добавленных провайдеров, ниже — сами провайдеры со своими ключами. Ключи пульт видит только хвостом.
 */
/**
 * `first` — шаг «Кто думает» первых шагов (docs/plans/onboarding.md): сначала провайдеры (вход и ключ), выбор модели —
 * когда есть из чего выбирать; «Что сейчас знает Мозг» там не нужно.
 */
export function BrainTab({ spend, first = false }: { spend?: preact.ComponentChildren; first?: boolean } = {}) {
    const panel = usePanel();
    const [view, setView] = useState<BrainView | null>(null);
    const [loadError, setLoadError] = useState<string | null>(null);
    const [notes, setNotes] = useState<Record<string, CardNote>>({});
    const [busy, setBusy] = useState<Record<string, boolean>>({});
    /** What the person picked in a list, while its answer is on its way (`apply`). */
    const [picked, setPicked] = useState<Record<string, string | undefined>>({});
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

    // Every 15 s and on the gateway's news (panel.version): the backup the gateway sets itself right after a choice
    // (`brain.changed`, block «Дом», default 6) and a helper's model coming or going (`helpers.*`) show at once.
    usePoll({ every: 15_000, version: panel.version, read: () => load() });

    const say = (id: string, note: CardNote) => setNotes((all) => ({ ...all, [id]: note }));
    const mark = (id: string, on: boolean) => setBusy((all) => ({ ...all, [id]: on }));

    /**
     * A choice to the gateway. `value` — what the person picked in the list: it stays shown while the answer is on its
     * way, instead of the old choice coming back until then.
     */
    const apply = async (profile: 'fast' | 'smart' | 'backup', choice: Parameters<typeof api.chooseBrain>[0], value?: string) => {
        mark(profile, true);
        if (value !== undefined) setPicked((all) => ({ ...all, [profile]: value }));
        say('choice', {});
        try {
            setView(await api.chooseBrain(choice));
            say('choice', { notice: tr.brain.applied });
        } catch (e) {
            say('choice', { error: problemOf(e) });
        } finally {
            mark(profile, false);
            setPicked((all) => ({ ...all, [profile]: undefined }));
        }
    };
    const choose = (profile: 'fast' | 'smart' | 'backup', pick: BrainPick | null, value?: string) =>
        apply(profile, { [profile]: pick }, value);
    /** What the list shows: the person's pick while its answer is on its way, otherwise the choice as read. */
    const shown = (profile: 'fast' | 'smart' | 'backup', read: string) => (busy[profile] && picked[profile] !== undefined ? picked[profile] : read);

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
    const signIn = useSignIn((id, title, note) => {
        // Ключ пришёл, но доступа пока нет (DuoWave): проверять нечем — сказать, что дальше, и перечитать карточку.
        if (note) {
            say(id, { warning: note });
            void load();
        } else void check(id, tr.brain.signedIn(title));
    });

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
    /**
     * Шлюз ставит «Запасную» сам, пока от неё не отказались (решение 18): тогда два пункта — «пусть выберет шлюз»
     * (`'auto'`: шлюз выбирает заново) и «не нужна» (`null`, шлюз больше не ставит). Шлюз без блока (0.13.0, 0.14.0 —
     * окно колонки) — один «не нужна», как в нём самом.
     */
    const autoBackup = !!choice.autoBackup;
    const envLabel = tr.brain.envLabel(env.title, env.models.fast);
    /** Куда уйдёт вопрос, если ответит запасная: её название и дома она или в интернете. */
    const backupWhere = !backup
        ? null
        : backup.provider === 'env'
          ? { title: env.title, cloud: env.cloud }
          : providers.find((provider) => provider.id === backup.provider) ?? null;
    /**
     * Helpers whose model answers (docs/plans/home-and-helpers.md, блок 1): what happens when one is away. Not this
     * computer's own model server: its card says whether it answers, and the gateway's note what answers when it does
     * not — «Этот компьютер не на связи» on the computer itself would say nothing true.
     */
    const helpers = [choice.fast, choice.smart]
        .map((pick) => providers.find((provider) => provider.id === pick?.provider)?.helper)
        .filter(
            (helper, i, all): helper is NonNullable<ProviderView['helper']> =>
                !!helper && !helper.builtin && all.findIndex((other) => other?.id === helper.id) === i,
        );
    const isUp = (helper: NonNullable<ProviderView['helper']>) => helper.state === 'up' || helper.state === 'self';
    const online = (helper: NonNullable<ProviderView['helper']>) =>
        isUp(helper) ? { text: tr.helpers.brainHelperOnline, cls: 'pill-own' } : { text: tr.helpers.brainHelperOffline, cls: 'pill-off' };
    /** The chosen model's card: «отвечает»; «выбрана» while its helper is away — the backup or the honest words answer. */
    const chosenPill = (helper: ProviderView['helper']) =>
        helper && !isUp(helper) ? { text: tr.brain.pillChosen, cls: 'pill-off' } : { text: tr.brain.pillAnswers, cls: 'pill-own' };
    /**
     * The backup stands in for a helper away only as the gateway judges it (stage 4): never one on that same helper —
     * it is away with it —, nor one the regulator keeps in. The latter the gateway's note names (`backupClosed`); a backup
     * beside a main model gives that note too, so the note is read as «kept in» only for a backup beside none.
     */
    const helperOf = (pick: BrainPick | null | undefined) => (pick ? providers.find((provider) => provider.id === pick.provider)?.helper : undefined);
    const backupHelper = helperOf(backup)?.id;
    const besideMain = !!backupHelper && [choice.fast, choice.smart].some((pick) => helperOf(pick)?.id === backupHelper);
    const backupShut = view.note?.kind === 'backupClosed' && !besideMain;
    const options = (profile: 'fast' | 'smart') => modelOptions(view, profile);

    const choiceCard = (
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
                            value={shown(profile, pickValue(profile === 'fast' ? choice.fast : choice.smart))}
                            disabled={!mayChoose || busy[profile] || (profile === 'smart' && !choice.fast)}
                            onChange={(e) => {
                                const value = (e.target as HTMLSelectElement).value;
                                void choose(profile, parsePick(value), value);
                            }}
                        >
                            {options(profile)}
                        </select>
                    </label>
                ))}
                {/* «Запасная» — кто думает, пока Турбо спит — живёт в «Доме» (решение владельца 16.11): здесь только
                    выбор моделей и одна строка туда. */}
                <p class="t-caption m-0 mt-4">
                    <button class="truth-more" onClick={() => panel.go('settings', 'helpers')}>
                        {tr.brain.backupMoved}
                    </button>
                    {backup && backupWhere ? ` · ${tr.brain.backupNow(backupWhere.title, backup.model)}` : ''}
                </p>
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
                                    class={`pill ${choice.effort === effort.id ? 'pill-pick' : 'pill-off'} cursor-pointer py-8 px-14 min-h-32`}
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
        </>
    );

    return (
        <>
            {!first && choiceCard}

            {/* «Расход» (docs/plans/router-and-works.md, этап 6б): сколько стоили модели — рядом с тем, где их выбирают. */}
            {spend}

            {!first && can('station.view') && <NowKnown />}

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
                    const access = provider.access;
                    // DuoWave: its access in words («Пробная неделя до 27 ноября») in place of the key's tail.
                    const keyLine = access
                        ? accessLine(access)
                        : account
                          ? account.signedIn
                              ? tr.brain.keyPlan(via?.with ?? '', account.who ?? '')
                              : tr.brain.needLogin
                          : provider.key.set
                            ? tr.brain.keyHint(provider.key.hint ?? '')
                            : provider.needsKey
                              ? tr.brain.noKey
                              : tr.brain.keyless;
                    const noKey = via ? tr.brain.noKeyVia(via.with) : tr.brain.noKeyPlain;
                    // A helper's model: the gateway keeps its row (address and key live in «Помощники»); this computer's
                    // own model server keeps its card, with whether it answers.
                    const mark = provider.helper;
                    const managed = !!mark && !mark.builtin && provider.preset === 'helper';
                    const MarkIcon = mark && managed ? placeIcon(mark) : null;
                    const accessNote = access && accessWarning(access);
                    const accountNote: CardNote | undefined = accessNote
                        ? { warning: accessNote }
                        : !account
                          ? undefined
                          : !account.signedIn
                            ? { warning: tr.brain.noLogin(via?.with ?? tr.brain.noLoginService) }
                            : account.problem === 'limit'
                              ? { warning: tr.brain.planLimit(via?.with ?? '') }
                              : undefined;
                    return (
                        <Card
                            key={provider.id}
                            icon={
                                MarkIcon ? (
                                    <span class="route-icon" aria-hidden="true" style={{ width: 38, height: 38, flex: 'none' }}>
                                        <MarkIcon size={19} />
                                    </span>
                                ) : (
                                    <ProviderIcon brand={provider.brand} />
                                )
                            }
                            title={provider.title}
                            pills={[
                                // Whose it is, by the colours of «Где сейчас»: at home lime, my server or cloud violet.
                                managed && mark
                                    ? { text: placeTitle(mark.kind), cls: mark.kind === 'home' ? 'pill-own' : 'pill-agent' }
                                    : provider.cloud
                                      ? { text: tr.brain.pillCloud, cls: 'pill-yandex' }
                                      : { text: tr.brain.pillHome, cls: 'pill-own' },
                                ...(mark ? [online(mark)] : []),
                                ...(answering ? [chosenPill(mark)] : []),
                                ...(backup?.provider === provider.id ? [{ text: tr.brain.pillBackup, cls: 'pill-off' }] : []),
                            ]}
                            description={managed ? `${words.text} · ${tr.helpers.brainManaged}` : `${words.text} · ${keyLine}`}
                            note={
                                notes[provider.id] ??
                                accountNote ??
                                (!managed && provider.needsKey && !provider.key.set ? { warning: noKey } : words.ok === false ? { error: words.text } : undefined)
                            }
                            action={null}
                        >
                            {mayManage && managed && (
                                <div class="flex items-center gap-6 flex-wrap pl-50">
                                    <Action label={tr.brain.check} tone="on" busy={busy[provider.id]} onClick={() => void check(provider.id)} />
                                    <Action label={tr.helpers.brainOpenHelpers} onClick={() => panel.go('settings', 'helpers')} />
                                </div>
                            )}
                            {mayManage && !managed && (
                                <div class="flex items-center gap-6 flex-wrap pl-50">
                                    <Action label={tr.brain.check} tone="on" busy={busy[provider.id]} onClick={() => void check(provider.id)} />
                                    {via && (
                                        <Action
                                            label={(account ? account.signedIn : provider.key.set) ? tr.brain.signInAgain : tr.brain.signInAction}
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
                                            {provider.brand === 'duowave' ? tr.brain.cabinetLower : tr.brain.limitsLower}
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
                {/* Первые шаги: пустой .env новичку ни о чём не говорит. */}
                {!(first && !env.usable) && (
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
                )}
            </section>

            {asking && <AskSheet onClose={() => setAsking(false)} />}
            {adding && (
                <AddProvider
                    // DuoWave спрятан, пока его не показали (shell/dev.ts); уже добавленный виден в списке провайдеров.
                    presets={presets}
                    firstOne={!choice.fast}
                    onClose={() => setAdding(false)}
                    onDone={(fresh) => {
                        if (fresh) setView(fresh);
                        else void load();
                    }}
                />
            )}
            {first && providers.length > 0 && choiceCard}
        </>
    );
}

/**
 * Шит «Добавить провайдера»: сначала какой (готовые и «свой»), потом ключ, адрес и поля пресета. «Проверить и
 * сохранить» сохраняет и сразу проверяет; если это первый провайдер и он работает — предложит отвечать им. У сервиса со
 * входом (OpenRouter) ключ можно не вставлять: «Войти через …» добавляет провайдера без ключа, ключ приходит входом.
 */
/**
 * Пункты списка модели: «как быстрая» / модель из .env / «не выбрана» у быстрой и умной; у «Запасной» — пункты без
 * модели из blankBackups и модель из .env; дальше модели каждого провайдера (выбранная, которой нет в его списке, —
 * первой).
 */
function modelOptions(view: BrainView, profile: 'fast' | 'smart' | 'backup') {
    const { providers, presets, choice, env } = view;
    const backup = choice.backup ?? null;
    const envLabel = tr.brain.envLabel(env.title, env.models.fast);
    const chosen = profile === 'fast' ? choice.fast : profile === 'smart' ? choice.smart : backup;
    return (
        <>
            {profile === 'backup' ? (
                blankBackups(choice).map((blank) => (
                    <option key={blank.value} value={blank.value}>
                        {blank.model ? tr.brain.optGatewayChose(blank.model) : tr.brain[blank.word]}
                    </option>
                ))
            ) : (
                <option value="">{profile === 'smart' ? tr.brain.optLikeFast : env.usable ? envLabel : tr.brain.optNotChosen}</option>
            )}
            {profile === 'backup' && env.usable && (
                <option value={pickValue({ provider: 'env', model: backup?.provider === 'env' ? backup.model : env.models.fast })}>{envLabel}</option>
            )}
            {providers.map((provider) => {
                const models = modelsOf(provider, presets);
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
}

/**
 * «Если Турбо уснёт» → «Думает» (страница «Дом», решение владельца 16.11): «Запасная» — кто ответит, пока модель Турбо
 * не на связи, и что станет с вопросом без неё. Прежде — в «Мозге», рядом с быстрой и умной; там теперь строка сюда.
 */
export function BackupChoice() {
    const panel = usePanel();
    const [view, setView] = useState<BrainView | null>(null);
    const [error, setError] = useState<string | null>(null);
    const [busy, setBusy] = useState(false);
    /** What the person picked, while its answer is on its way. */
    const [picked, setPicked] = useState<string | undefined>(undefined);
    const [note, setNote] = useState<CardNote>({});
    const mayChoose = can('models.manage');
    // Every 15 s and on the gateway's news: the backup the gateway sets itself right after a choice shows at once.
    usePoll({
        every: 15_000,
        version: panel.version,
        read: () =>
            api.brain().then(
                (fresh) => {
                    setView(fresh);
                    setError(null);
                },
                (e) => setError(problemOf(e)),
            ),
    });
    if (!view)
        return (
            <p class="w-cap m-0" role={error ? 'alert' : undefined}>
                {error ?? tr.brain.askingGateway}
            </p>
        );
    const { choice, providers } = view;
    const backup = choice.backup ?? null;
    const where = backupPlace(view);
    const helperOf = (pick: BrainPick | null | undefined) => (pick ? providers.find((provider) => provider.id === pick.provider)?.helper : undefined);
    // Турбо, чья модель отвечает (кроме сервера моделей этого компьютера: о нём — заметка шлюза).
    const turbos = [choice.fast, choice.smart]
        .map((pick) => helperOf(pick))
        .filter((helper, i, all): helper is NonNullable<ProviderView['helper']> => !!helper && !helper.builtin && all.findIndex((other) => other?.id === helper.id) === i);
    const isUp = (helper: NonNullable<ProviderView['helper']>) => helper.state === 'up' || helper.state === 'self';
    const backupHelper = helperOf(backup)?.id;
    const besideMain = !!backupHelper && [choice.fast, choice.smart].some((pick) => helperOf(pick)?.id === backupHelper);
    const shut = view.note?.kind === 'backupClosed' && !besideMain;
    const apply = async (value: string): Promise<void> => {
        setBusy(true);
        setPicked(value);
        setNote({});
        try {
            setView(await api.chooseBrain({ backup: backupOf(value, !!choice.autoBackup) }));
            setNote({ notice: tr.brain.applied });
        } catch (e) {
            setNote({ error: problemOf(e) });
        } finally {
            setBusy(false);
            setPicked(undefined);
        }
    };
    return (
        <div class="flex flex-col gap-6">
            <select
                class="field max-w-full min-w-0"
                aria-label={tr.brain.backup}
                value={busy && picked !== undefined ? picked : backupShown(choice)}
                disabled={!mayChoose || busy}
                onChange={(e) => void apply((e.target as HTMLSelectElement).value)}
            >
                {modelOptions(view, 'backup')}
            </select>
            {/* Without a backup: through a speaker — a station, or a computer that answers one — an «Алиса + ваш ИИ»
                question goes to Alice; in «Микрофон» there is no Alice. */}
            {turbos.map((helper) => (
                <p key={helper.id} class="w-cap m-0">
                    <b>{helper.name}</b> · {isUp(helper) ? tr.helpers.brainHelperOnline : tr.helpers.brainHelperOffline}.{' '}
                    {backup && where && backupHelper !== helper.id && !shut
                        ? tr.helpers.brainIfAway(where.title, backup.model)
                        : byMode(tr.helpers.brainIfAwayNoneStation, tr.helpers.brainIfAwayNoneComputer)(
                              backup && where ? tr.helpers.brainBackupIdle : tr.helpers.brainNoBackup,
                          )}
                </p>
            ))}
            {backupIdle(choice, [choice.fast, choice.smart].some((pick) => !!helperOf(pick))) && <p class="w-cap m-0">{tr.brain.backupGatewayIdle}</p>}
            {where && <p class="w-cap m-0">{tr.brain.backupExplain(where.title, where.cloud)}</p>}
            {view.note && (
                <p class={`w-cap m-0${view.note.kind === 'backupAuto' ? '' : ' c-pill-yandex-fg'}`} role="status">
                    {view.note.text}
                </p>
            )}
            {note.error && (
                <p class="w-cap m-0 c-dot-offline" role="alert">
                    {note.error}
                </p>
            )}
            {note.notice && (
                <p class="w-cap m-0" role="status">
                    {note.notice}
                </p>
            )}
        </div>
    );
}

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
                <div class="win-group">
                    {presets.map((entry) => (
                        <button key={entry.id} class="win-row win-choice text-left" onClick={() => start(entry)}>
                            <ProviderIcon brand={entry.brand} size={32} />
                            <span class="win-row-main">
                                <span class="win-row-t">{entry.title}</span>
                                <span class="win-row-s">{entry.description}</span>
                            </span>
                            {entry.local && <span class="pill pill-own">{tr.brain.pillHome}</span>}
                        </button>
                    ))}
                </div>
            </Sheet>
        );
    }

    const words = result ? checkWords(result.check) : null;
    const keyless = !!preset.signIn?.keyless;
    const spin = (on: boolean) => on && <span class="spinner spinner-sm" aria-hidden="true" />;
    const signInButton = (main: boolean) =>
        preset.signIn && (
            <button
                type="button"
                class={main ? 'btn btn-main' : 'btn btn-plain btn-sm self-start'}
                disabled={busy || signIn.starting}
                aria-busy={signIn.starting || undefined}
                onClick={() => preset.signIn && beginSignIn(preset, preset.signIn.with)}
            >
                {spin(signIn.starting)}
                {signIn.starting ? tr.brain.opening : (preset.signIn.label ?? tr.brain.signInWith(preset.signIn.with))}
            </button>
        );
    // Внизу: «Назад» к выбору и главное — «Проверить и сохранить» или, когда ключ не нужен, «Войти через …»;
    // после проверки — «Отвечать им» (первый провайдер). Закрыть — крестиком.
    const footer = result ? (
        firstOne && result.check.ok && result.check.model ? (
            <button class="btn btn-main" disabled={busy} aria-busy={busy || undefined} onClick={() => void useIt()}>
                {spin(busy)}
                {tr.brain.useIt(result.check.model)}
            </button>
        ) : undefined
    ) : (
        <>
            <button class="btn btn-plain" onClick={() => setPreset(null)}>
                {tr.brain.back}
            </button>
            {keyless ? (
                signInButton(true)
            ) : (
                <button
                    class="btn btn-main"
                    disabled={busy || (preset.needsKey && !key.trim()) || (preset.editableUrl && !url.trim())}
                    aria-busy={busy || undefined}
                    onClick={() => void save()}
                >
                    {spin(busy)}
                    {busy ? tr.brain.checking : tr.brain.checkAndSave}
                </button>
            )}
        </>
    );
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
            icon={<ProviderIcon brand={preset.brand} size={44} />}
            onClose={onClose}
            footer={footer}
        >
            {!result && (
                <>
                    <p class="win-lead">{preset.description}</p>
                    {preset.signIn && (
                        <WinSection group>
                            <div class="win-row is-col">
                                {!keyless && signInButton(false)}
                                {keyless ? (
                                    <span class="win-row-s">
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
                                    <span class="win-row-s">{tr.brain.keyCreated(preset.signIn.with)}</span>
                                )}
                                {signIn.steps}
                            </div>
                        </WinSection>
                    )}
                    <form
                        class="flex flex-col gap-14"
                        onSubmit={(e) => {
                            e.preventDefault();
                            void save();
                        }}
                    >
                        {preset.editableUrl && (
                            <label class="flex flex-col gap-6">
                                <span class="win-label">{tr.brain.address}</span>
                                <input class="field" value={url} placeholder="http://localhost:8000/v1" onInput={(e) => setUrl((e.target as HTMLInputElement).value)} />
                            </label>
                        )}
                        {!keyless && (
                            <label class="flex flex-col gap-6">
                                <span class="win-label">{preset.needsKey ? tr.brain.keyField : tr.brain.keyFieldOptional}</span>
                                <input
                                    class="field"
                                    type="password"
                                    autocomplete="off"
                                    value={key}
                                    onInput={(e) => setKey((e.target as HTMLInputElement).value)}
                                />
                                {preset.keyUrl && (
                                    <a class="win-hint c-accent-text self-start" href={preset.keyUrl} target="_blank" rel="noopener noreferrer">
                                        {tr.brain.whereKey}
                                    </a>
                                )}
                            </label>
                        )}
                        {keyless && preset.keyUrl && (
                            <a class="win-hint c-accent-text self-start" href={preset.keyUrl} target="_blank" rel="noopener noreferrer">
                                {tr.brain.whereKey}
                            </a>
                        )}
                        {(preset.fields ?? []).map((field) => (
                            <label key={field.id} class="flex flex-col gap-6">
                                <span class="win-label">{field.title}</span>
                                <input
                                    class="field"
                                    value={fields[field.id] ?? ''}
                                    placeholder={field.placeholder}
                                    onInput={(e) => setFields((all) => ({ ...all, [field.id]: (e.target as HTMLInputElement).value }))}
                                />
                            </label>
                        ))}
                        <label class="flex flex-col gap-6">
                            <span class="win-label">{tr.brain.titleField}</span>
                            <input class="field" value={title} placeholder={preset.title} onInput={(e) => setTitle((e.target as HTMLInputElement).value)} />
                        </label>
                    </form>
                    <p class="win-note">{keyless ? byMode(tr.brain.loginStoredSpeaker, tr.brain.loginStoredComputer) : tr.brain.keyStored}</p>
                </>
            )}
            {result && words && (
                <p role="status" class={words.ok ? 'win-callout is-calm m-0' : 'win-callout m-0'}>
                    {words.ok ? tr.brain.savedOk(words.text) : tr.brain.savedFailed(words.text)}
                </p>
            )}
            {error && (
                <p role="alert" class="win-callout m-0">
                    {error}
                </p>
            )}
        </Sheet>
    );
}
