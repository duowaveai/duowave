// Выбор ИИ одной логикой (docs/plans/start-desk.md): пять путей — DuoWave (наши модели, docs/plans/duowave-connect.md),
// OpenRouter, подписка ChatGPT, свой ключ, модель на компьютере — те же вызовы шлюза, что у «Мозга» (screens/brain.tsx): добавить провайдера, войти, проверить, выбрать
// модели «за человека». Им пользуются шаг «Кто думает» первых шагов (onb/brain.tsx) и витрина стола (showcase.tsx).

import { useEffect, useRef, useState } from 'preact/hooks';

import { BrainPick, BrainView, ProviderAccess, ProviderCheck, ProviderPreset, ProviderView, api, can, problemOf } from '../api';
import { formatDate, tr } from '../i18n';
import { usePoll } from '../poll';
import { usePanel } from '../shell/context';
import { useSignIn } from '../ui/sign-in';

/** Пути доски по порядку карточек: DuoWave первым, виден всем (владелец 22.11 — без скрытой отметки). */
export type Path = 'duowave' | 'openrouter' | 'chatgpt' | 'key' | 'local';
export const PATHS: Path[] = ['duowave', 'openrouter', 'chatgpt', 'key', 'local'];
/** Пути со входом через сервис. */
export type SignInPath = 'duowave' | 'openrouter' | 'chatgpt';

/** Сервисы «Своего ключа» (пресеты шлюза) и модели на компьютере. */
export const KEY_PRESETS = ['claude', 'openai', 'deepseek', 'gemini', 'groq', 'mistral', 'yandexgpt'];
export const LOCAL_PRESETS = ['ollama', 'lmstudio'];
/** Сервис, которого нет среди пресетов: свой адрес, OpenAI-совместимый. Его и Anthropic-совместимый путь «Свой ключ» тоже считает своими. */
const CUSTOM_PRESET = 'custom-openai';
const CUSTOM_PRESETS = [CUSTOM_PRESET, 'custom-anthropic'];
/** Короче — ключ ещё вписывают: поле адреса не мигает на первых знаках. */
const KEY_MIN = 8;


/**
 * Сервис по виду ключа: dw- — DuoWave, sk-ant- — Anthropic, sk-or- — OpenRouter, sk- и 32 шестнадцатеричных знака — DeepSeek,
 * прочие sk- — OpenAI, AIza — Gemini, gsk_ — Groq, AQVN — Yandex Cloud, 32 буквы и цифры — Mistral. Не узнали — null:
 * ключ не уходит наугад в чужой сервис.
 */
function keyService(raw: string): string | null {
    const key = raw.trim();
    if (/^dw-/.test(key)) return 'duowave';
    if (/^sk-ant-/.test(key)) return 'claude';
    if (/^sk-or-/.test(key)) return 'openrouter';
    if (/^sk-[0-9a-f]{32}$/.test(key)) return 'deepseek';
    if (/^sk-/.test(key)) return 'openai';
    if (/^AIza/.test(key)) return 'gemini';
    if (/^gsk_/.test(key)) return 'groq';
    if (/^AQVN/.test(key)) return 'yandexgpt';
    if (/^[A-Za-z0-9]{32}$/.test(key)) return 'mistral';
    return null;
}

/** Доступ DuoWave строкой карточки: «Пробная неделя до 27 ноября», «доступа пока нет». */
export function accessLine(access: ProviderAccess): string {
    if (!access.active) return tr.brain.accessNone;
    const date = access.endsAt ? formatDate(access.endsAt, { day: 'numeric', month: 'long' }) : null;
    return date ? tr.brain.accessUntil(access.plan ?? '', date) : tr.brain.accessPlan(access.plan ?? '');
}

/** Что просит последний отказ DuoWave или его доступ: предупреждение карточки; нет — всё в порядке. */
export function accessWarning(access: ProviderAccess): string | undefined {
    switch (access.problem) {
        case 'revoked':
            return tr.brain.accessRevoked;
        case 'model':
            return tr.brain.accessModel;
        case 'ended':
            return tr.brain.accessEnded;
        case 'quota':
            return tr.brain.accessQuota;
        case 'busy':
            return tr.brain.accessBusy;
    }
    return access.active ? undefined : tr.brain.accessNoneNote;
}

/** Работает ли провайдер: вошли (подписка), есть ключ, на компьютере назвал модели — и проверка не упала, а сервис не
 * сказал, что доступа нет (DuoWave). */
function ready(provider: ProviderView): boolean {
    if (provider.check && !provider.check.ok) return false;
    if (provider.access && (!provider.access.active || provider.access.problem === 'revoked')) return false;
    if (provider.account) return provider.account.signedIn;
    if (provider.local) return !!provider.check?.ok || provider.models.length > 0;
    return provider.key.set || (!provider.needsKey && !!provider.check?.ok);
}

/** Самая новая модель из списка по образцам, по порядку образцов: «anthropic/claude-haiku-5.5» раньше «…-4.5». */
function newest(models: string[], patterns: RegExp[]): string | null {
    for (const pattern of patterns) {
        const found = models.filter((model) => pattern.test(model)).sort((a, b) => b.localeCompare(a, undefined, { numeric: true }));
        if (found.length) return found[0];
    }
    return null;
}

/** OpenRouter не советует моделей: быструю и умную ищем в его списке у известных семейств. */
const OPENROUTER_FAST = [/^anthropic\/claude-haiku-[\d.]+$/, /^openai\/gpt-[\d.]+-mini$/, /^google\/gemini-[\d.]+-flash$/];
const OPENROUTER_SMART = [/^anthropic\/claude-sonnet-[\d.]+$/, /^openai\/gpt-[\d.]+$/, /^google\/gemini-[\d.]+-pro$/];

/**
 * «Выбрали за вас»: быстрая и умная для провайдера. Советы пресета (Claude, YandexGPT), у OpenRouter — свежие из
 * известных семейств, иначе та, что ответила на проверку (как «Отвечать ею» у «Добавить провайдера»). Умной нет —
 * «как быстрая».
 */
function picksFor(provider: ProviderView, preset: ProviderPreset | undefined, checked?: string): { fast: BrainPick | null; smart: BrainPick | null } {
    const at = (model: string | null | undefined): BrainPick | null => (model ? { provider: provider.id, model } : null);
    if (preset?.suggested) {
        const smart = preset.suggested.smart !== preset.suggested.fast ? preset.suggested.smart : null;
        return { fast: at(preset.suggested.fast), smart: at(smart) };
    }
    if (provider.preset === 'openrouter') {
        const fast = newest(provider.models, OPENROUTER_FAST) ?? checked ?? provider.models[0];
        const smart = newest(provider.models, OPENROUTER_SMART);
        return { fast: at(fast), smart: at(smart !== fast ? smart : null) };
    }
    return { fast: at(checked ?? provider.check?.model ?? provider.models[0]), smart: null };
}

/** Выбор ИИ: что подключено, какой путь отмечен, вход, ключ, поиск модели на компьютере. */
export function useBrainConnect() {
    const [view, setView] = useState<BrainView | null>(null);
    const [path, setPath] = useState<Path>('duowave');
    /** Какой путь сейчас проверяет провайдера (после входа или ключа). */
    const [checking, setChecking] = useState<{ path: Path; title: string } | null>(null);
    const [problem, setProblem] = useState<{ path: Path; text: string } | null>(null);
    const [key, setKey] = useState('');
    const [fields, setFields] = useState<Record<string, string>>({});
    /** Провайдер, добавленный «Своим ключом» здесь: его карточка — «Ключ … работает». */
    const [keyAdded, setKeyAdded] = useState<string | null>(null);
    /** Поиск модели на компьютере: идёт, не нашёл, нашёл. */
    const [local, setLocal] = useState<'searching' | 'none' | 'found' | null>(null);
    /** Провайдер, в который сейчас входят: вход знает только путь, а проверять надо его. */
    const signing = useRef<{ provider: ProviderView; path: Path } | null>(null);
    const alive = useRef(true);
    const viewRef = useRef<BrainView | null>(null);
    viewRef.current = view;

    useEffect(() => {
        alive.current = true;
        return () => {
            alive.current = false;
        };
    }, []);

    /** Идёт своё дело хука (проверка ключа, вход): тогда «выбрать за человека» не вмешивается. */
    const acting = useRef(false);
    /** Первый взгляд прочитан: какой путь уже отвечает — тот и отмечен (вернулись на шаг), иначе DuoWave — первый. */
    const looked = useRef(false);
    /** «Выбрать за человека» уже было: один раз на открытие. */
    const adopted = useRef(false);

    const load = async (): Promise<BrainView | null> => {
        try {
            const fresh = await api.brain();
            if (!alive.current) return fresh;
            setView(fresh);
            if (!looked.current) {
                looked.current = true;
                const answering = fresh.providers.find((provider) => provider.id === fresh.choice.fast?.provider);
                if (answering) setPath(pathOf(answering));
            }
            void adopt(fresh);
            return fresh;
        } catch (e) {
            if (alive.current) setProblem({ path, text: problemOf(e) });
            return null;
        }
    };

    // Сразу и по новостям шлюза (`brain.changed` растит panel.version): провайдера добавили в «Мозге», пока стол открыт, —
    // витрина и первые шаги видят это, а не снимок с открытия.
    const panel = usePanel();
    usePoll({ version: panel.version, read: () => load() });

    /**
     * Провайдер работает, а модель не выбрана: добавили его в «Персонаж» → «Мозг» и закрыли лист, не нажав «Отвечать
     * им». Шлюз без выбора отвечает только моделью из .env — для человека ИИ нет, хотя он его подключил, и шаг 1 не
     * засчитан. Выбираем за него, как после входа здесь («Выбрали за вас»): один раз и только тому, кто выбирает модели.
     */
    const adopt = async (fresh: BrainView) => {
        if (adopted.current || acting.current || signing.current || searching.current) return;
        if (fresh.choice.fast || fresh.env.usable || !can('models.manage')) return;
        const provider = fresh.providers.find((entry) => ready(entry) && !!(entry.check?.model ?? entry.models[0]));
        if (!provider) return;
        adopted.current = true;
        const which = pathOf(provider);
        setPath(which);
        try {
            await answerWith(provider, fresh);
        } catch (e) {
            if (alive.current) setProblem({ path: which, text: problemOf(e) });
        }
    };

    const presetOf = (id: string, from: BrainView | null = viewRef.current) => from?.presets.find((preset) => preset.id === id);

    /** Провайдер пути, который уже работает. */
    const workingOn = (which: Path, from: BrainView | null = view): ProviderView | undefined => {
        if (!from) return undefined;
        const answering = from.choice.fast?.provider;
        const fits = (provider: ProviderView) =>
            which === 'key'
                ? provider.id === keyAdded || KEY_PRESETS.includes(provider.preset) || CUSTOM_PRESETS.includes(provider.preset)
                : which === 'local'
                  ? LOCAL_PRESETS.includes(provider.preset)
                  : provider.preset === which;
        const all = from.providers.filter((provider) => fits(provider) && ready(provider));
        return all.find((provider) => provider.id === answering) ?? all[0];
    };

    /** Отвечать этим провайдером: быстрая и умная — «Выбрали за вас». */
    const answerWith = async (provider: ProviderView, from: BrainView, checked?: string) => {
        const picks = picksFor(provider, presetOf(provider.preset, from), checked);
        if (!picks.fast) return;
        const same = from.choice.fast?.provider === picks.fast.provider && from.choice.fast.model === picks.fast.model;
        if (same && (from.choice.smart?.model ?? null) === (picks.smart?.model ?? null)) return;
        const fresh = await api.chooseBrain({ fast: picks.fast, smart: picks.smart });
        if (alive.current) setView(fresh);
    };

    /**
     * После входа или ключа: проверить провайдера, затем отвечать им. true — работает и отвечает. `given` — модель,
     * которую назвал человек (свой адрес): проверяется она.
     */
    const settle = async (target: Pick<ProviderView, 'id' | 'preset' | 'title'>, which: Path, given?: string): Promise<boolean> => {
        const { id } = target;
        setChecking({ path: which, title: target.title });
        setProblem(null);
        acting.current = true;
        try {
            let model: string | undefined = given;
            // OpenRouter моделей не советует: сначала его список, проверяем ту, что выберем быстрой.
            if (!model && target.preset === 'openrouter') {
                const listed = await api.providerModels(id).catch(() => ({ models: [] as string[] }));
                model = newest(listed.models, OPENROUTER_FAST) ?? undefined;
            }
            const check: ProviderCheck = await api.checkProvider(id, model);
            const fresh = await api.brain();
            if (!alive.current) return false;
            setView(fresh);
            const provider = fresh.providers.find((entry) => entry.id === id);
            if (!check.ok || !provider) {
                setProblem({ path: which, text: check.problem ?? tr.brain.notWorking });
                return false;
            }
            // DuoWave шлюз уже поставил сам — модели назвал сервис (`defaults`); выбирать «за человека» поверх не надо.
            if (provider.preset !== 'duowave' || fresh.choice.fast?.provider !== id) await answerWith(provider, fresh, check.model);
            return true;
        } catch (e) {
            if (alive.current) setProblem({ path: which, text: problemOf(e) });
            return false;
        } finally {
            acting.current = false;
            if (alive.current) setChecking(null);
        }
    };

    /**
     * «Войти через …» (DuoWave, OpenRouter, ChatGPT): как у «Добавить провайдера» — провайдер без ключа, потом страница
     * сервиса. После входа DuoWave может сказать, что доступа пока нет: тогда проверять нечем — это и показываем.
     */
    const signIn = useSignIn((_target, _title, note) => {
        const done = signing.current;
        if (!done) return;
        if (note) {
            setProblem({ path: done.path, text: note });
            void load();
        } else void settle(done.provider, done.path);
    });
    const beginSignIn = (which: SignInPath) => {
        const preset = presetOf(which);
        if (!preset || signIn.starting) return;
        setProblem(null);
        signIn.start(which, preset.signIn?.with ?? preset.title, async () => {
            const existing = viewRef.current?.providers.find((provider) => provider.preset === which);
            const provider = existing ?? (await api.addProvider({ preset: which }));
            signing.current = { provider, path: which };
            if (!existing) void load();
            return api.signInProvider(provider.id);
        });
    };

    /**
     * Код приглашения DuoWave: шлюз запоминает его и сам прикладывает к входам DuoWave, пока один не пройдёт (подарок
     * даётся только при самом первом входе на сайте). Пустой — забыть.
     */
    const [inviteBusy, setInviteBusy] = useState(false);
    const saveInvite = async (code: string): Promise<boolean> => {
        setProblem(null);
        setInviteBusy(true);
        try {
            const fresh = await api.setInvite(code.trim() || null);
            if (alive.current) setView(fresh);
            return true;
        } catch (e) {
            if (alive.current) setProblem({ path: 'duowave', text: problemOf(e) });
            return false;
        } finally {
            if (alive.current) setInviteBusy(false);
        }
    };

    /**
     * «Проверить» у своего ключа: сервис по ключу, провайдер (или новый ключ прежнему), проверка, выбор. Не узнали
     * сервис по ключу — человек вписывает его адрес (OpenAI-совместимый) и, если сервис сам моделей не называет, модель:
     * провайдер «Свой, OpenAI-совместимый» добавляется прямо здесь, без похода в «Мозг».
     */
    const [keyUrl, setKeyUrl] = useState('');
    const [keyModel, setKeyModel] = useState('');
    const service = keyService(key);
    const keyPreset = service ? presetOf(service) : undefined;
    const keyFields = keyPreset?.fields ?? [];
    /** Ключ вписан, а сервис по нему не узнан: нужен адрес. */
    const keyUnknown = !keyPreset && key.trim().length >= KEY_MIN && !!presetOf(CUSTOM_PRESET, view);
    const urlOk = /^https?:\/\/[^\s/]+/i.test(keyUrl.trim());
    const keyReady = !!key.trim() && (keyPreset ? keyFields.every((field) => (fields[field.id] ?? '').trim()) : keyUnknown && urlOk);
    const saveKey = async () => {
        if (!key.trim()) return;
        if (!keyPreset && !(keyUnknown && urlOk)) {
            setProblem({ path: 'key', text: keyUnknown ? tr.onbBrain.keyNeedsUrl : tr.onbBrain.keyUnknown });
            return;
        }
        const baseUrl = keyUrl.trim().replace(/\/+$/, '');
        const title = keyPreset ? keyPreset.title : hostOf(baseUrl);
        setProblem(null);
        setChecking({ path: 'key', title });
        acting.current = true;
        try {
            const given = keyPreset ? { key: key.trim(), ...(keyFields.length ? { options: fields } : {}) } : { key: key.trim(), baseUrl, title };
            // Свой адрес: прежняя строка — добавленная здесь же (поправили адрес или ключ) или с тем же адресом.
            const existing = viewRef.current?.providers.find((provider) =>
                keyPreset
                    ? provider.preset === keyPreset.id
                    : provider.preset === CUSTOM_PRESET && (provider.id === keyAdded || provider.baseUrl?.replace(/\/+$/, '') === baseUrl),
            );
            const preset = keyPreset ? keyPreset.id : CUSTOM_PRESET;
            const provider = existing ? await api.updateProvider(existing.id, given) : await api.addProvider({ preset, ...given });
            if (!alive.current) return;
            setKeyAdded(provider.id);
            if (await settle(provider, 'key', keyPreset ? undefined : keyModel.trim() || undefined)) {
                setKey('');
                setKeyUrl('');
                setKeyModel('');
            }
        } catch (e) {
            if (alive.current) {
                setProblem({ path: 'key', text: problemOf(e) });
                setChecking(null);
            }
        } finally {
            acting.current = false;
        }
    };

    /**
     * Модель на этом компьютере: шлюз сам спрашивает Ollama и LM Studio по их обычным адресам. Нет такого провайдера —
     * добавляем на время вопроса и убираем, если не ответил; ответил — остаётся и отвечает.
     */
    const searching = useRef(false);
    const findLocal = async (quiet: boolean) => {
        if (searching.current) return;
        searching.current = true;
        if (!quiet) setLocal('searching');
        try {
            for (const id of LOCAL_PRESETS) {
                const preset = presetOf(id);
                if (!preset) continue;
                const existing = viewRef.current?.providers.find((provider) => provider.preset === id);
                let provider: ProviderView;
                try {
                    provider = existing ?? (await api.addProvider({ preset: id, ...(preset.baseUrl ? { baseUrl: preset.baseUrl } : {}) }));
                } catch {
                    continue;
                }
                const models = await api
                    .providerModels(provider.id)
                    .then((listed) => listed.models)
                    .catch(() => [] as string[]);
                if (models.length) {
                    const fresh = await api.brain();
                    if (!alive.current) return;
                    setView(fresh);
                    const found = fresh.providers.find((entry) => entry.id === provider.id);
                    if (found) await answerWith(found, fresh, models[0]);
                    if (alive.current) setLocal('found');
                    return;
                }
                if (!existing) await api.removeProvider(provider.id).catch(() => undefined);
            }
            if (alive.current) setLocal('none');
        } catch (e) {
            if (alive.current) {
                setProblem({ path: 'local', text: problemOf(e) });
                setLocal('none');
            }
        } finally {
            searching.current = false;
        }
    };

    useEffect(() => {
        if (path !== 'local' || !view) return;
        if (workingOn('local')) {
            setLocal('found');
            return;
        }
        // Один поиск при открытии: каждый поиск добавляет и убирает провайдера в базе шлюза — по таймеру его не крутим,
        // ещё раз — кнопкой «Проверить снова».
        if (local === null) void findLocal(false);
    }, [path, !!view, local, !!workingOn('local')]);

    // Модель на компьютере стала основной — шлюз чуть позже сам ставит облачную «Запасную» (умолчание 6 блока «Дом»,
    // по событию выбора, а список облака может прийти и ещё позже). Пока путь открыт, выбор перечитывается: карточка
    // говорит, куда уйдёт вопрос, когда модель молчит, а не «вопросы не уходят из дома».
    const localFast =
        path === 'local' &&
        !!view &&
        view.providers.some((provider) => provider.id === view.choice.fast?.provider && LOCAL_PRESETS.includes(provider.preset));
    useEffect(() => {
        if (!localFast) return undefined;
        const reread = () =>
            void api
                .brain()
                .then((fresh) => alive.current && setView(fresh))
                .catch(() => undefined);
        const soon = window.setTimeout(reread, 1500);
        const every = window.setInterval(reread, 10_000);
        return () => {
            window.clearTimeout(soon);
            window.clearInterval(every);
        };
    }, [localFast, view?.choice.fast?.provider, view?.choice.fast?.model]);

    /** «Без запасной»: вопросы остаются дома; шлюз запоминает отказ и сам её больше не ставит. */
    const noBackup = async () => {
        setProblem(null);
        try {
            const fresh = await api.chooseBrain({ backup: null });
            if (alive.current) setView(fresh);
        } catch (e) {
            if (alive.current) setProblem({ path: 'local', text: problemOf(e) });
        }
    };

    /** Отметить путь. Его провайдер уже работает — отвечать им (радио доски = кто думает). */
    const choosePath = (which: Path) => {
        setPath(which);
        setProblem(null);
        const provider = workingOn(which);
        if (view && provider && view.choice.fast?.provider !== provider.id) {
            void answerWith(provider, view).catch((e) => setProblem({ path: which, text: problemOf(e) }));
        }
    };

    /** ИИ есть: выбрана модель или работает модель из .env. */
    const answering = !!view && (!!view.choice.fast || view.env.usable);
    return {
        view,
        load,
        path,
        choosePath,
        checking,
        problem,
        key,
        setKey,
        fields,
        setFields,
        keyPreset,
        keyFields,
        keyUnknown,
        keyUrl,
        setKeyUrl,
        keyModel,
        setKeyModel,
        keyReady,
        saveKey,
        local,
        findLocal,
        signIn,
        beginSignIn,
        paths: PATHS,
        invite: !!view?.invite,
        inviteBusy,
        saveInvite,
        workingOn,
        answering,
        noBackup,
    };
}

export type BrainConnect = ReturnType<typeof useBrainConnect>;

/** Имя своего сервиса по адресу: «api.together.xyz»; не разобрали — адрес как есть. */
function hostOf(url: string): string {
    try {
        return new URL(url).host || url;
    } catch {
        return url;
    }
}

export /** Путь карточки, которому принадлежит провайдер. */
function pathOf(provider: ProviderView): Path {
    if (provider.preset === 'duowave') return 'duowave';
    if (provider.preset === 'openrouter') return 'openrouter';
    if (provider.preset === 'chatgpt') return 'chatgpt';
    if (LOCAL_PRESETS.includes(provider.preset) || provider.local) return 'local';
    return 'key';
}
