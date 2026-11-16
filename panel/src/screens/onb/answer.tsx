// Первые шаги → «Кто отвечает на колонке?» — доски Answer / AnswerOnly холста «Онбординг» 1:1 (на узком — одной
// колонкой, как PhoneAnswer). Режимы «Как раньше · Вместе · Своё» — тот же режим колонки, что переключатель на столе
// (api.mode / api.setMode; DuoWave уже на колонке — через проход помощника к её шлюзу). «Чей ИИ отвечает» — те же
// вызовы, что у «Мозга» (screens/brain.tsx): добавить провайдера, войти через OpenRouter, ключ, модель на компьютере,
// проверить и выбрать. У «Только колонка» модель живёт на колонке — ключ туда же (отсюда предупреждение доски про SSH).
// Пока колонка перезагружается после «Дальше, не ждать», здесь видно, пришла ли она (onb/speaker.tsx).

import { useEffect, useRef, useState } from 'preact/hooks';

import {
    BrainPick,
    BrainView,
    ModeView,
    PanelMode,
    ProviderCheck,
    ProviderInput,
    ProviderPreset,
    ProviderView,
    SignInStart,
    SignInStatus,
    api,
    can,
    problemOf,
    stationCall,
} from '../../api';
import { tr } from '../../i18n';
import { usePanel } from '../../shell/context';
import { NoArrival, linkActive, restoreLink, useSpeakerLink } from './speaker';
import { useOnboarding, useStepNav } from './nav';
import '../../ui/onboarding-canvas.css';
import '../../ui/onb-connect.css';

/** Куда идут вызовы: шлюз этого пульта или шлюз колонки через проход помощника (`/station/<колонка>/…`). */
interface Gate {
    mode: () => Promise<ModeView>;
    setMode: (mode: PanelMode) => Promise<ModeView>;
    brain: () => Promise<BrainView>;
    addProvider: (input: ProviderInput) => Promise<ProviderView>;
    updateProvider: (id: string, input: ProviderInput) => Promise<ProviderView>;
    removeProvider: (id: string) => Promise<void>;
    checkProvider: (id: string) => Promise<ProviderCheck>;
    signInProvider: (id: string) => Promise<SignInStart>;
    signInStatus: (id: string) => Promise<SignInStatus>;
    completeSignIn: (id: string, text: string) => Promise<{ title: string }>;
    chooseBrain: (choice: { fast?: BrainPick | null; smart?: BrainPick | null }) => Promise<BrainView>;
}

const direct: Gate = {
    mode: () => api.mode(),
    setMode: (mode) => api.setMode(mode),
    brain: () => api.brain(),
    addProvider: (input) => api.addProvider(input),
    updateProvider: (id, input) => api.updateProvider(id, input),
    removeProvider: (id) => api.removeProvider(id),
    checkProvider: (id) => api.checkProvider(id),
    signInProvider: (id) => api.signInProvider(id),
    signInStatus: (id) => api.signInStatus(id),
    completeSignIn: (id, text) => api.completeSignIn(id, text),
    chooseBrain: (choice) => api.chooseBrain(choice),
};

function viaStation(key: string): Gate {
    const enc = encodeURIComponent;
    return {
        mode: () => stationCall<ModeView>(key, 'GET', '/api/v1/station/mode'),
        setMode: (mode) => stationCall<ModeView>(key, 'POST', '/api/v1/station/mode', { mode }),
        brain: () => stationCall<BrainView>(key, 'GET', '/api/v1/brain'),
        addProvider: (input) => stationCall<ProviderView>(key, 'POST', '/api/v1/brain/providers', input),
        updateProvider: (id, input) => stationCall<ProviderView>(key, 'PATCH', `/api/v1/brain/providers/${enc(id)}`, input),
        removeProvider: (id) => stationCall<void>(key, 'DELETE', `/api/v1/brain/providers/${enc(id)}`),
        checkProvider: (id) => stationCall<ProviderCheck>(key, 'POST', `/api/v1/brain/providers/${enc(id)}/check`, {}),
        signInProvider: (id) => stationCall<SignInStart>(key, 'POST', `/api/v1/brain/providers/${enc(id)}/signin`),
        signInStatus: (id) => stationCall<SignInStatus>(key, 'GET', `/api/v1/oauth/${enc(id)}`),
        completeSignIn: (id, text) => stationCall<{ title: string }>(key, 'POST', '/api/v1/oauth/complete', { id, text }),
        chooseBrain: (choice) => stationCall<BrainView>(key, 'PUT', '/api/v1/brain/choice', choice),
    };
}

const MODES: PanelMode[] = ['relay', 'gateway', 'own'];

type ModelPath = 'openrouter' | 'key' | 'local';

/**
 * Сервис по виду ключа (как у шага «Кто думает»): sk-ant- — Anthropic, sk-or- — OpenRouter, sk- и 32 шестнадцатеричных
 * знака — DeepSeek, прочие sk- — OpenAI, AIza — Gemini, gsk_ — Groq, AQVN — Yandex Cloud. Не узнали — null.
 */
function keyService(raw: string): string | null {
    const key = raw.trim();
    if (/^sk-ant-/.test(key)) return 'claude';
    if (/^sk-or-/.test(key)) return 'openrouter';
    if (/^sk-[0-9a-f]{32}$/.test(key)) return 'deepseek';
    if (/^sk-/.test(key)) return 'openai';
    if (/^AIza/.test(key)) return 'gemini';
    if (/^gsk_/.test(key)) return 'groq';
    if (/^AQVN/.test(key)) return 'yandexgpt';
    return null;
}

/** Какой путь у провайдера: вход OpenRouter, модель на компьютере или свой ключ. */
function pathOf(provider: ProviderView | undefined): ModelPath | null {
    if (!provider) return null;
    if (provider.preset === 'openrouter') return 'openrouter';
    return provider.local ? 'local' : 'key';
}

const IconWarn = () => (
    <svg class="ico ico-sm" viewBox="0 0 24 24" aria-hidden="true">
        <path d="M12 7v6M12 16.5v.5" />
    </svg>
);

/** Шаг «Кто отвечает» первых шагов: доска Answer (flow = station) или AnswerOnly (flow = only). */
export function AnswerStep() {
    const t = tr.onbConnect;
    const { choice, leave } = useOnboarding();
    const only = choice === 'station';
    const panel = usePanel();
    const live = useSpeakerLink();
    // DuoWave уже на колонке — пульт стал помощником: режим (и у «Только колонка» модель) — у шлюза колонки.
    const stationKey = panel.role.role === 'helper' ? panel.role.station?.key : undefined;
    const modeGate = stationKey ? viaStation(stationKey) : direct;
    const modelGate = only && stationKey ? viaStation(stationKey) : direct;
    const mayPick = can('station.mode');
    const mayModel = can('connectors.manage') && can('models.manage');

    const [mode, setMode] = useState(1);
    const [current, setCurrent] = useState<PanelMode | null>(null);
    const [view, setView] = useState<BrainView | null>(null);
    const [path, setPath] = useState<ModelPath>('openrouter');
    const [busy, setBusy] = useState(false);
    const [note, setNote] = useState<{ text: string; bad?: boolean } | null>(null);
    const [keyOpen, setKeyOpen] = useState(false);
    const [key, setKey] = useState('');
    const [service, setService] = useState('');
    const [signin, setSignin] = useState<{ start: SignInStart; provider: string } | null>(null);
    const [pasted, setPasted] = useState('');
    const [restoring, setRestoring] = useState(false);
    const picked = useRef(false);

    // Что сейчас: режим колонки и выбранная модель. Режим «Как раньше» — сразу советуем «Вместе», как доска.
    useEffect(() => {
        let alive = true;
        modeGate
            .mode()
            .then((view) => {
                if (!alive) return;
                setCurrent(view.mode);
                if (!picked.current && view.mode !== 'relay' && !(only && view.mode === 'own')) setMode(MODES.indexOf(view.mode));
            })
            .catch(() => undefined);
        modelGate
            .brain()
            .then((fresh) => {
                if (!alive) return;
                setView(fresh);
                const chosen = pathOf(fresh.providers.find((provider) => provider.id === fresh.choice.fast?.provider));
                if (chosen && (chosen !== 'local' || !only)) setPath(chosen);
            })
            .catch(() => undefined);
        return () => {
            alive = false;
        };
    }, [stationKey]);

    const chosenProvider = view?.providers.find((provider) => provider.id === view.choice.fast?.provider);
    const signed = !!chosenProvider && pathOf(chosenProvider) === path;
    const presetOf = (id: string): ProviderPreset | undefined => view?.presets.find((preset) => preset.id === id);

    /** Провайдер готов: проверить, выбрать его модель быстрой и сказать, кто отвечает. */
    const settle = async (provider: ProviderView) => {
        const check = await modelGate.checkProvider(provider.id);
        if (!check.ok) throw new Error(check.problem ?? tr.connect.problemOther);
        const model = check.model ?? presetOf(provider.preset)?.suggested?.fast ?? provider.models[0];
        if (!model) throw new Error(tr.connect.problemOther);
        setView(await modelGate.chooseBrain({ fast: { provider: provider.id, model } }));
        setNote({ text: t.modelWorks(model) });
    };

    const work = async (job: () => Promise<void>) => {
        setBusy(true);
        setNote(null);
        try {
            await job();
        } catch (e) {
            setNote({ text: problemOf(e), bad: true });
        } finally {
            setBusy(false);
        }
    };

    // Вход идёт на странице сервиса: раз в две секунды спрашиваем шлюз, как дела.
    useEffect(() => {
        if (!signin) return;
        let alive = true;
        const id = window.setInterval(async () => {
            try {
                const status = await modelGate.signInStatus(signin.start.id);
                if (!alive) return;
                if (status.state === 'done') {
                    window.clearInterval(id);
                    setSignin(null);
                    const provider = (await modelGate.brain()).providers.find((entry) => entry.id === signin.provider);
                    if (provider) void work(() => settle(provider));
                } else if (status.state === 'failed') {
                    setSignin(null);
                    setNote({ text: status.message, bad: true });
                }
            } catch {
                // Связь мигнула — спросим ещё раз.
            }
        }, 2000);
        return () => {
            alive = false;
            window.clearInterval(id);
        };
    }, [signin?.start.id]);

    /** «Войти через OpenRouter»: окно открывается сразу по нажатию, адрес страницы приходит от шлюза. */
    const signIn = () => {
        const page = window.open('about:blank', '_blank');
        if (page) page.opener = null;
        void work(async () => {
            try {
                const provider = view?.providers.find((entry) => entry.preset === 'openrouter') ?? (await modelGate.addProvider({ preset: 'openrouter' }));
                const start = await modelGate.signInProvider(provider.id);
                setPasted('');
                setSignin({ start, provider: provider.id });
                if (page && !page.closed) page.location.href = start.url;
            } catch (e) {
                page?.close();
                throw e;
            }
        });
    };

    const finishSignIn = () =>
        signin &&
        void work(async () => {
            await modelGate.completeSignIn(signin.start.id, pasted.trim());
            const provider = (await modelGate.brain()).providers.find((entry) => entry.id === signin.provider);
            setSignin(null);
            if (provider) await settle(provider);
        });

    const keyPresets = (view?.presets ?? []).filter((preset) => preset.needsKey && !preset.local && !preset.signIn?.keyless && preset.id !== 'openrouter');
    const guessed = keyService(key);
    const keyTarget = service || (guessed && (guessed === 'openrouter' || keyPresets.some((preset) => preset.id === guessed)) ? guessed : '');

    const saveKey = () =>
        void work(async () => {
            if (!keyTarget) throw new Error(t.keyUnknown);
            const existing = view?.providers.find((provider) => provider.preset === keyTarget);
            const provider = existing
                ? await modelGate.updateProvider(existing.id, { key: key.trim() })
                : await modelGate.addProvider({ preset: keyTarget, key: key.trim() });
            await settle(provider);
            setKey('');
            setKeyOpen(false);
        });

    /** «Выбрать модель»: Ollama или LM Studio на этом компьютере — та, что ответит на проверку. */
    const pickLocal = () =>
        void work(async () => {
            const locals = (view?.presets ?? []).filter((preset) => preset.local);
            for (const preset of locals) {
                const existing = view?.providers.find((provider) => provider.preset === preset.id);
                const provider = existing ?? (await modelGate.addProvider({ preset: preset.id }));
                const check = await modelGate.checkProvider(provider.id).catch(() => null);
                if (check?.ok && check.model) {
                    setView(await modelGate.chooseBrain({ fast: { provider: provider.id, model: check.model } }));
                    setNote({ text: t.modelWorks(check.model) });
                    return;
                }
                // Не ответила — не оставляем пустую карточку в «Мозге».
                if (!existing) await modelGate.removeProvider(provider.id).catch(() => undefined);
            }
            throw new Error(t.localNone);
        });

    const act = () => {
        if (path === 'openrouter') signIn();
        else if (path === 'key') setKeyOpen((open) => !open);
        else pickLocal();
    };

    const restore = async () => {
        setRestoring(true);
        try {
            await restoreLink();
        } catch (e) {
            setNote({ text: problemOf(e), bad: true });
        } finally {
            setRestoring(false);
        }
    };

    const latest = useRef({ mode, current, modeGate });
    latest.current = { mode, current, modeGate };
    useStepNav(
        {
            label: mode === 0 ? t.finish : t.next,
            disabled: busy,
            onNext: async () => {
                const now = latest.current;
                const want = MODES[now.mode];
                if (mayPick && want !== now.current) {
                    const switched = await now.modeGate.setMode(want);
                    setCurrent(switched.mode);
                }
                // «Как раньше» — первые шаги кончаются здесь: на стол, как доска (DeskStation).
                if (now.mode === 0) {
                    leave();
                    return false;
                }
            },
        },
        [mode, busy, current, stationKey],
    );

    const alice = 'c-out';
    const ours = 'c-own';
    const modes = [
        {
            title: t.modeRelay,
            sub: t.modeRelaySub,
            ex: [
                { say: t.exWeather, who: t.whoAlice, cls: alice },
                { say: t.exWidget, who: t.whoCant, cls: 'c-sub' },
            ],
        },
        {
            title: t.modeGateway,
            best: true,
            sub: t.modeGatewaySub,
            ex: [
                { say: t.exJazz, who: t.whoMusic, cls: alice },
                { say: t.exWeatherWidget, who: t.whoOurs, cls: ours },
            ],
        },
        {
            title: t.modeOwn,
            sub: t.modeOwnSub,
            ex: [
                { say: t.exHowAreYou, who: t.whoOurs, cls: ours },
                { say: t.exAlarm, who: t.whoOurs, cls: ours },
            ],
        },
    ];
    const paths: Array<{ id: ModelPath; title: string; sub: string }> = [
        { id: 'openrouter', title: t.optOpenRouter, sub: t.optOpenRouterSub },
        { id: 'key', title: t.optKey, sub: t.optKeySub },
        ...(only ? [] : [{ id: 'local' as const, title: t.optLocal, sub: t.optLocalSub }]),
    ];
    const actions: Record<ModelPath, string> = { openrouter: t.actSignIn, key: t.actKey, local: t.actLocal };
    const needModel = mode !== 0;

    return (
        <div class="onb-cx onb-cx-ans">
            <div class="onb-cx-head">
                <h1 class="h2">
                    {t.answerTitle1}
                    <br /> {t.answerTitle2}
                </h1>
                <p class="lead">{t.answerLead}</p>
            </div>

            <div class="onb-cx-modes" role="radiogroup" aria-label={t.modelTitle}>
                {modes.map((entry, at) => {
                    const locked = only && at === 2;
                    return (
                        <button
                            key={entry.title}
                            type="button"
                            class={`opt onb-cx-mode${mode === at ? ' on' : ''}${locked ? ' off' : ''}`}
                            aria-pressed={mode === at}
                            aria-disabled={locked || !mayPick}
                            onClick={() => {
                                if (locked || !mayPick) return;
                                picked.current = true;
                                setMode(at);
                            }}
                        >
                            <span class="onb-cx-mode-top">
                                <span class="radio" />
                                <span class="opt-title">{entry.title}</span>
                                {entry.best && <span class="chip chip-lime">{t.best}</span>}
                                {locked && <span class="chip chip-lock">{t.needsPc}</span>}
                            </span>
                            <span class="opt-sub" style={{ margin: 0 }}>
                                {entry.sub}
                            </span>
                            <span class="onb-cx-ex">
                                {entry.ex.map((line) => (
                                    <span key={line.say}>
                                        <span class="q">{line.say}</span>
                                        <span class={line.cls}>{line.who}</span>
                                    </span>
                                ))}
                            </span>
                        </button>
                    );
                })}
            </div>

            {needModel && (
                <div class="card card-tight onb-cx-model">
                    <div class="onb-cx-model-row">
                        <span class="h3">{t.modelTitle}</span>
                        <div class="onb-cx-model-opts">
                            {paths.map((entry) => (
                                <button
                                    key={entry.id}
                                    type="button"
                                    class={`opt${path === entry.id ? ' on' : ''}`}
                                    aria-pressed={path === entry.id}
                                    disabled={busy}
                                    onClick={() => {
                                        setPath(entry.id);
                                        setNote(null);
                                        setSignin(null);
                                        setKeyOpen(false);
                                    }}
                                >
                                    <span class="radio" />
                                    <span class="opt-body">
                                        <span class="opt-title">{entry.title}</span>
                                        <span class="opt-sub">{entry.sub}</span>
                                    </span>
                                </button>
                            ))}
                        </div>
                        <button type="button" class="dw-btn dw-btn-lg dw-btn-dark" disabled={busy || !mayModel || !view} onClick={act}>
                            {busy && !keyOpen ? t.checkingModel : signed ? t.actDone : actions[path]}
                        </button>
                    </div>

                    {signin && (
                        <div class="onb-cx-form">
                            <span class="small" style={{ flexBasis: '100%' }}>
                                {signin.start.mode === 'code'
                                    ? tr.ui.signInSayCode(t.optOpenRouter)
                                    : signin.start.mode === 'address'
                                      ? tr.ui.signInSayAddress(t.optOpenRouter)
                                      : tr.ui.signInSayHere(t.optOpenRouter)}{' '}
                                <a href={signin.start.url} target="_blank" rel="noopener noreferrer">
                                    {tr.ui.signInOpen(t.optOpenRouter)}
                                </a>
                            </span>
                            <input
                                class="input"
                                autocomplete="off"
                                aria-label={signin.start.mode === 'code' ? tr.ui.signInCodeFrom(t.optOpenRouter) : tr.ui.signInAddressAfter}
                                placeholder={
                                    signin.start.mode === 'code'
                                        ? tr.ui.signInCodeFrom(t.optOpenRouter)
                                        : signin.start.mode === 'address'
                                          ? tr.ui.signInAddressAfter
                                          : tr.ui.signInPlaceholderHere
                                }
                                value={pasted}
                                onInput={(e) => setPasted((e.target as HTMLInputElement).value)}
                            />
                            <button type="button" class="dw-btn dw-btn-light" disabled={!pasted.trim() || busy} onClick={finishSignIn}>
                                {tr.ui.signInFinish}
                            </button>
                        </div>
                    )}

                    {path === 'key' && keyOpen && (
                        <form
                            class="onb-cx-form"
                            onSubmit={(e) => {
                                e.preventDefault();
                                if (key.trim()) saveKey();
                            }}
                        >
                            <input
                                class="input"
                                type="password"
                                autocomplete="off"
                                aria-label={t.keyLabel}
                                placeholder={t.keyPlaceholder}
                                value={key}
                                onInput={(e) => setKey((e.target as HTMLInputElement).value)}
                            />
                            {key.trim() && !guessed && (
                                <select class="input" aria-label={t.keyService} value={service} onChange={(e) => setService((e.target as HTMLSelectElement).value)}>
                                    <option value="">{t.keyService}</option>
                                    {keyPresets.map((preset) => (
                                        <option key={preset.id} value={preset.id}>
                                            {preset.title}
                                        </option>
                                    ))}
                                </select>
                            )}
                            <button type="submit" class="dw-btn dw-btn-light" disabled={!key.trim() || !keyTarget || busy}>
                                {busy ? t.checkingModel : t.save}
                            </button>
                        </form>
                    )}

                    {only && path === 'key' && (
                        <p class="small onb-cx-note">
                            <span class="mark mark-warn onb-cx-mark-sm">
                                <IconWarn />
                            </span>
                            {t.keyWarn}
                        </p>
                    )}
                    {note && (
                        <p class={`small onb-cx-p${note.bad ? ' onb-cx-bad' : ''}`} role={note.bad ? 'alert' : 'status'}>
                            {note.text}
                        </p>
                    )}
                    <p class="small onb-cx-p">{path === 'local' ? t.noteLocal : t.noteDefault}</p>
                </div>
            )}
            {!needModel && <p class="small onb-cx-p">{t.noModel}</p>}

            {/* «Ждать не нужно»: колонка ещё в пути — строка; пришла — «на связи»; не пришла за 3 минуты — что делать. */}
            {live && linkActive(live) && (
                <p class="small onb-cx-status" role="status">
                    <span class="dot dot-busy" aria-hidden="true" />
                    {live.phase === 'moving' ? t.linkMoving : live.phase === 'restoring' ? t.restoring : t.rebooting}
                </p>
            )}
            {live?.phase === 'done' && (
                <p class="small onb-cx-status" role="status">
                    <span class="dot dot-ok" aria-hidden="true" />
                    {t.linkOnline(live.name)}
                    {live.moveError !== undefined ? ` · ${t.moveFailed(live.moveError)}` : ''}
                </p>
            )}
            {live?.phase === 'restored' && (
                <p class="small onb-cx-status" role="status">
                    <span class="dot" aria-hidden="true" />
                    {t.restored}
                </p>
            )}
            {(live?.phase === 'timeout' || live?.phase === 'waiting') && (
                <div style={{ maxWidth: '560px' }}>
                    <NoArrival busy={restoring} onRestore={() => void restore()} />
                </div>
            )}
        </div>
    );
}
