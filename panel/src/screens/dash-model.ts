import { BrainStatus, BrainView, ConnectorView, ModelsView, PanelMode, PersonaInfo, TtsCard } from '../api';
import type { AvailabilityView, FeatureState, Placement } from '../availability';
import { Scenario, Solved, solve } from '../truth';
import { executorOf } from '../executor';
import { formatList, tr } from '../i18n';

// Дашборд без отрисовки: что на самом деле работает в каждом узле орбиты при этом режиме и откуда, каким
// цветом его показать и что сказать в подписи. Источник — API шлюза; макет — renderVals в
// docs/design/glass/source/GlassDash.dc.html.

/** Цвет узла — кто делает работу (DESIGN.md: лайм дома, янтарь Яндекс и облако, фиолет помощник, сланец — нет). */
export type Tone = 'own' | 'cloud' | 'yandex' | 'agent' | 'off';
export type NodeKey = 'brain' | 'think' | 'persona' | 'ear' | 'voice';
/** Узлы на орбите — полукругом: слух, мозг, голос. Мышление и характер открываются из шитов мозга и голоса. */
export type OrbitKey = Extract<NodeKey, 'brain' | 'ear' | 'voice'>;
/** Путь вопроса по орбите после нажатия на колонку: слух → мозг → голос. */
export type TripStage = 'idle' | 'listen' | 'think' | 'speak' | 'done';
export type Thinking = 'fast' | 'smart' | 'deep';

export const NODE_LABELS: Record<NodeKey, string> = {
    get brain() {
        return tr.dashboard.nodeBrain;
    },
    get think() {
        return tr.dashboard.nodeThink;
    },
    get persona() {
        return tr.dashboard.nodePersona;
    },
    get ear() {
        return tr.dashboard.nodeEar;
    },
    get voice() {
        return tr.dashboard.nodeVoice;
    },
};

/** Подпись к цвету: облако и Яндекс одного янтаря (The Outside-Is-Amber Rule), но называются по-своему. */
export const WHERE: Record<Tone, string> = {
    get own() {
        return tr.dashboard.whereOwn;
    },
    get cloud() {
        return tr.dashboard.whereCloud;
    },
    get yandex() {
        return tr.dashboard.whereYandex;
    },
    get agent() {
        return tr.dashboard.whereAgent;
    },
    get off() {
        return tr.dashboard.whereOff;
    },
};

/** Цвет полосы исполнителя: кольцо колонки, комета на орбите, обводка горящего узла. */
export const LINE: Record<Tone, string> = {
    own: 'var(--bar-own)',
    cloud: 'var(--bar-yandex)',
    yandex: 'var(--bar-yandex)',
    agent: 'var(--bar-agent)',
    off: 'var(--bar-off)',
};

/** Класс оттенка в CSS: облако красится как Яндекс. */
export const toneClass = (tone: Tone): string => `tone-${tone === 'cloud' ? 'yandex' : tone}`;

export interface NodeView {
    key: NodeKey;
    value: string;
    tone: Tone;
    /** Строка для шита: что это и где работает. */
    detail: string;
}

export interface DashData {
    mode: PanelMode;
    /** Маршруты из /health: есть, когда колонка говорит со шлюзом. */
    routes?: Record<string, string>;
    brain?: BrainStatus;
    /** Провайдеры и выбор моделей из пульта (право смотреть подключения); без него шит мозга короче. */
    brainView?: BrainView | null;
    models?: ModelsView | null;
    persona?: PersonaInfo | null;
    connectors?: ConnectorView[] | null;
    /** Что где доступно (список шлюза): на колонке слух и голос — у компьютера, в облаке или у Яндекса. */
    availability?: AvailabilityView | null;
    /** Режим «Микрофон»: говорят микрофоном этого компьютера, колонки и Яндекса нет. */
    computer?: boolean;
}

export const THINKING: Record<Thinking, { name: string; sub: string; pill: string }> = {
    fast: {
        get name() {
            return tr.dashboard.thinkingFastName;
        },
        get sub() {
            return tr.dashboard.thinkingFastSub;
        },
        get pill() {
            return tr.dashboard.thinkingFastPill;
        },
    },
    smart: {
        get name() {
            return tr.dashboard.thinkingSmartName;
        },
        get sub() {
            return tr.dashboard.thinkingSmartSub;
        },
        get pill() {
            return tr.dashboard.thinkingSmartPill;
        },
    },
    deep: {
        get name() {
            return tr.dashboard.thinkingDeepName;
        },
        get sub() {
            return tr.dashboard.thinkingDeepSub;
        },
        get pill() {
            return tr.dashboard.thinkingDeepPill;
        },
    },
};

/** «claude-haiku-5-5» → «Haiku 5.5»; не Claude — как есть. */
export function claudeName(id: string): string {
    const match = /^claude-([a-z]+)-(\d+)(?:-(\d{1,2}))?(?:-\d{8})?$/.exec(id);
    if (!match) return id;
    return `${match[1][0].toUpperCase()}${match[1].slice(1)} ${match[2]}${match[3] ? `.${match[3]}` : ''}`;
}

const FAMILIES: Array<[RegExp, string]> = [
    [/^(gpt|o\d|chatgpt)/i, 'ChatGPT'],
    [/^llama/i, 'Llama'],
    [/^qwen/i, 'Qwen'],
    [/^gemma/i, 'Gemma'],
    [/^gemini/i, 'Gemini'],
    [/^(mistral|mixtral|ministral)/i, 'Mistral'],
    [/^deepseek/i, 'DeepSeek'],
    [/^phi/i, 'Phi'],
    [/^grok/i, 'Grok'],
];

/** Имя модели OpenAI-совместимого сервиса для узла: семья, если узнаём, иначе имя без тега («llama3.1:8b» → «Llama»). */
export function familyName(model: string): string {
    const base = model.split('/').pop() ?? model;
    for (const [pattern, name] of FAMILIES) if (pattern.test(base)) return name;
    return base.split(':')[0] || tr.dashboard.modelWord;
}

/** Глубина мышления по настройке шлюза: effort Claude и вторая, умная модель для сложного. */
export function thinkingOf(brain: BrainStatus): Thinking {
    if (brain.effort === 'high' || brain.effort === 'xhigh' || brain.effort === 'max') return 'deep';
    if (brain.models.fast !== brain.models.smart || brain.effort === 'medium') return 'smart';
    return 'fast';
}

/** Наша модель, как её назвать и где она: для узла и для строки выбора в шите. */
export function ourBrain(brain: BrainStatus | undefined): { name: string; detail: string; tone: Tone; usable: boolean } {
    if (!brain) return { name: tr.dashboard.ownModel, detail: tr.dashboard.ownModelUnknown, tone: 'off', usable: false };
    const chosen = brain.profiles?.fast;
    if (brain.source === 'panel' && chosen) {
        const harder = brain.profiles?.smart;
        const named = (model: string, kind: string) => (kind === 'anthropic' ? claudeName(model) : model);
        const second =
            harder && (harder.model !== chosen.model || harder.provider !== chosen.provider) ? tr.dashboard.harder(named(harder.model, harder.kind)) : '';
        return {
            name: chosen.kind === 'anthropic' ? 'Claude' : familyName(chosen.model),
            detail: `${named(chosen.model, chosen.kind)} · ${chosen.title}${second}`,
            tone: chosen.cloud ? 'cloud' : 'own',
            usable: true,
        };
    }
    const { fast, smart } = brain.models;
    switch (brain.provider) {
        case 'none':
            return { name: tr.dashboard.ownModel, detail: tr.dashboard.ownModelNotSet, tone: 'off', usable: false };
        case 'echo':
            return { name: tr.dashboard.echoName, detail: tr.dashboard.echoDetail, tone: 'own', usable: true };
        case 'anthropic':
            return {
                name: 'Claude',
                detail: `${claudeName(fast)} · Anthropic${smart !== fast ? tr.dashboard.harder(claudeName(smart)) : ''}`,
                tone: 'cloud',
                usable: true,
            };
        default:
            return {
                name: fast ? familyName(fast) : tr.dashboard.ownModel,
                detail: `${fast || tr.dashboard.llmModelNotSet} · ${brain.cloud ? tr.dashboard.whereCloud : tr.dashboard.whereOwn}`,
                tone: brain.cloud ? 'cloud' : 'own',
                usable: !!fast,
            };
    }
}

/** Мозг — кто отвечает на разговор: маршрут «Разговор» под режимом, а при своём шаге — наша модель. */
export function brainNode(data: DashData): NodeView {
    // Режим «Микрофон»: разговор без колонки идёт «Мозгу» всегда — маршрут «Разговор» про колонку и Яндекс.
    const executor = data.computer ? 'own' : executorOf(data.routes?.chat ?? 'own', data.mode);
    if (executor === 'yandex') return { key: 'brain', value: tr.dashboard.alice, tone: 'yandex', detail: tr.dashboard.yandexAnswers };
    if (executor === 'off')
        return {
            key: 'brain',
            value: tr.dashboard.none,
            tone: 'off',
            detail: data.mode === 'own' ? tr.dashboard.brainOffRelayOnly : tr.dashboard.brainOffRoute,
        };
    const ours = ourBrain(data.brain);
    return { key: 'brain', value: ours.usable ? ours.name : tr.dashboard.none, tone: ours.usable ? ours.tone : 'off', detail: ours.detail };
}

export function thinkNode(data: DashData, brain: NodeView): NodeView {
    if (brain.tone === 'yandex') return { key: 'think', value: tr.dashboard.atYandex, tone: 'yandex', detail: tr.dashboard.aliceThinks };
    if (brain.tone === 'off' || !data.brain) return { key: 'think', value: tr.dashboard.none, tone: 'off', detail: tr.dashboard.brainNotChosen };
    if (data.brain.provider === 'echo') return { key: 'think', value: tr.dashboard.none, tone: 'off', detail: tr.dashboard.echoNoThink };
    const { fast, smart } = data.brain.models;
    const second = smart !== fast ? tr.dashboard.harderLine(data.brain.provider === 'anthropic' ? claudeName(smart) : smart) : tr.dashboard.oneModel;
    const effort = data.brain.provider === 'anthropic' ? `effort ${data.brain.effort ?? 'low'} · ` : '';
    return { key: 'think', value: THINKING[thinkingOf(data.brain)].name, tone: brain.tone, detail: `${effort}${second}` };
}

export function personaNode(data: DashData, brain: NodeView): NodeView {
    if (brain.tone === 'yandex') return { key: 'persona', value: tr.dashboard.atYandex, tone: 'yandex', detail: tr.dashboard.aliceOwnPersona };
    if (brain.tone === 'off') return { key: 'persona', value: tr.dashboard.none, tone: 'off', detail: tr.dashboard.noBrainNoPersona };
    const info = data.persona;
    if (!info) return { key: 'persona', value: '—', tone: 'off', detail: tr.dashboard.personaUnread };
    const preset = info.presets.find((entry) => entry.id === info.activePreset);
    if (preset) return { key: 'persona', value: preset.title, tone: 'own', detail: preset.description };
    return info.source === 'file'
        ? { key: 'persona', value: tr.dashboard.fromFile, tone: 'own', detail: 'DIALOG_SYSTEM_PROMPT_FILE' }
        : { key: 'persona', value: tr.dashboard.custom, tone: 'own', detail: tr.dashboard.writtenInPanel };
}

const cloudOf = (data: DashData, id: string): boolean =>
    data.connectors?.find((entry) => entry.manifest.id === id)?.manifest.cloud ?? false;

/** Короткое имя карточки для узла: «GigaAM v3 — конец-в-конец» → «GigaAM v3». */
export const shortTitle = (title: string): string => title.split(' — ')[0];

/** Слух — что распознаёт речь сейчас: модель, которую загрузил сервер, а не только выбранная. */
export function earNode(data: DashData): NodeView {
    if (data.mode === 'relay') return { key: 'ear', value: tr.dashboard.yandexWord, tone: 'yandex', detail: tr.dashboard.stationListensToYandex };
    const view = data.models;
    if (!view) return { key: 'ear', value: '—', tone: 'off', detail: tr.dashboard.modelsUnread };
    const tone: Tone = cloudOf(data, 'asr') ? 'cloud' : 'own';
    const active = view.asr.find((card) => card.active);
    if (active) return { key: 'ear', value: shortTitle(active.title), tone, detail: active.description };
    if (view.asrLoaded) return { key: 'ear', value: view.asrLoaded, tone, detail: tr.dashboard.modelNotInCatalog };
    const waiting = view.asr.find((card) => card.pending);
    return { key: 'ear', value: waiting ? shortTitle(waiting.title) : tr.dashboard.none, tone: 'off', detail: tr.dashboard.recognitionServerSilent };
}

/** Голоса Silero по-русски: в каталоге они латиницей (имена — в словаре языка). */

export function voiceName(card: TtsCard, voiceId?: string): string {
    if (card.engine !== 'silero') return card.title.replace(/\s*\(Piper\)\s*$/i, '');
    const name = card.voices.find((voice) => voice.id === voiceId)?.name ?? voiceId?.split('/').pop() ?? '';
    return tr.dashboard.sileroNames[name] ?? (name || shortTitle(card.title));
}

/** Голос — что звучит: с режимом (в «Своём» голос Алисы уступает нашему) и с тем, отвечает ли голосовой сервер. */
export function voiceNode(data: DashData): NodeView {
    if (data.mode === 'relay') return { key: 'voice', value: tr.dashboard.alice, tone: 'yandex', detail: tr.dashboard.yandexVoice };
    const view = data.models;
    if (!view) return { key: 'voice', value: '—', tone: 'off', detail: tr.dashboard.modelsUnread };
    // Режим «Микрофон»: голос Алисы идёт через колонку, без неё он молчит — нужен свой.
    if (data.computer && view.voice === 'yandex') return { key: 'voice', value: tr.dashboard.none, tone: 'off', detail: tr.dashboard.ownVoiceNotChosen };
    if (view.speaking === 'yandex') return { key: 'voice', value: tr.dashboard.alice, tone: 'yandex', detail: tr.dashboard.yandexVoice };
    if (!view.ownVoiceServer) return { key: 'voice', value: tr.dashboard.none, tone: 'off', detail: tr.dashboard.ownVoiceServerNotSet };
    const card = view.tts.find((entry) => entry.active && entry.engine !== 'yandex');
    if (!view.ttsUp)
        return {
            key: 'voice',
            value: card ? voiceName(card, view.ttsActive.voiceId) : tr.dashboard.none,
            tone: 'off',
            detail: tr.dashboard.voiceServerSilent,
        };
    const tone: Tone = cloudOf(data, 'tts') ? 'cloud' : 'own';
    if (!card) return { key: 'voice', value: tr.dashboard.ownShort, tone, detail: tr.dashboard.voiceFromEnv };
    // Выбран, но голосовой сервер его не загрузил (не скачан или скачан после его старта): так он не звучит.
    if (!card.loaded)
        return {
            key: 'voice',
            value: voiceName(card, view.ttsActive.voiceId),
            tone: 'off',
            detail: card.installed ? tr.dashboard.notLoadedRestart : tr.dashboard.notDownloaded,
        };
    const engine = card.group === 'own' ? tr.dashboard.ownEngine(card.own?.engineTitle ?? card.engine) : card.engine === 'silero' ? shortTitle(card.title) : 'Piper';
    const instead = view.voice === 'yandex' ? tr.dashboard.insteadOfAlice : '';
    return { key: 'voice', value: voiceName(card, view.ttsActive.voiceId), tone, detail: `${engine}${instead}` };
}

/**
 * Узел на колонке по списку шлюза: где работает сейчас — у компьютера (дома, лайм: он дома), в облаке, у Яндекса, или
 * нигде. На компьютере узлы — из его моделей, как раньше.
 */
export function placedNode(key: 'ear' | 'voice', feature: FeatureState | undefined, fallback: NodeView): NodeView {
    if (!feature) return fallback;
    switch (feature.runs) {
        case 'homeServer':
            return { key, value: tr.dashboard.computerWord, tone: 'own', detail: key === 'ear' ? tr.dashboard.homeServerEar : tr.dashboard.homeServerVoice };
        case 'cloud':
            return { key, value: tr.dashboard.cloudWord, tone: 'cloud', detail: feature.state === 'fallback' ? feature.reason : key === 'ear' ? tr.dashboard.cloudEar : tr.dashboard.cloudVoice };
        case 'yandex':
            return { key, value: key === 'ear' ? tr.dashboard.yandexWord : tr.dashboard.alice, tone: 'yandex', detail: feature.reason };
        default:
            return feature.state === 'off' ? { key, value: tr.dashboard.none, tone: 'off', detail: feature.reason } : fallback;
    }
}

/** Узлы, как их видят модели и список шлюза, — до сверки с тем, кто на деле отвечает. */
function rawNodesOf(data: DashData): Record<NodeKey, NodeView> {
    const brain = brainNode(data);
    const onStation = data.availability && data.availability.placement !== 'pc' && data.mode !== 'relay';
    const feature = (id: string) => data.availability?.features.find((entry) => entry.id === id);
    return {
        brain,
        think: thinkNode(data, brain),
        persona: personaNode(data, brain),
        ear: onStation ? placedNode('ear', feature('asr.own'), earNode(data)) : earNode(data),
        voice: onStation ? placedNode('voice', feature('voice.own'), voiceNode(data)) : voiceNode(data),
    };
}

const working = (entry: ConnectorView): boolean => entry.status.state === 'ready' || entry.status.state === 'configured';

/**
 * Условия для расчёта «кто слышит, думает, говорит» (truth.ts) — из того, что шлюз сказал о себе. `placement` —
 * другое размещение для окна выбора: что будет, если перенести; ПК при этом на связи (выбирают на нём).
 * `assumeCloud` — считать облако подключённым, есть ли ключи на самом деле: так окно «Где работает DuoWave»
 * показывает, как схема заработает с облачным распознаванием и голосом, а не только текущее состояние. Расчёт
 * главного экрана его не передаёт — там только правда.
 */
export function scenarioOf(data: DashData, placement?: Placement, assumeCloud = false): Scenario {
    const view = data.availability;
    const here = view?.placement ?? 'pc';
    const target = placement ?? here;
    const feature = (id: string) => view?.features.find((entry) => entry.id === id);
    const cloud = (id: string) =>
        data.connectors?.some((entry) => entry.manifest.id === id && entry.manifest.cloud && working(entry)) ?? false;
    const brain = brainNode(data);
    const where: Scenario['brain']['where'] =
        brain.tone === 'yandex' ? 'yandex' : brain.tone === 'off' ? 'none' : brain.tone === 'own' ? 'pc' : 'cloud';
    // Пульт ПК: сервер распознавания и голоса ПК; пока модели не прочитаны — считаем, что работают.
    const pcEar = here === 'pc' ? !data.models || earNode(data).tone !== 'off' : true;
    const pcVoice = here === 'pc' ? !data.models || voiceNode(data).tone !== 'off' : true;
    return {
        placement: target,
        mode: data.mode,
        pcUp: here === 'pc' || placement !== undefined ? true : view?.homeServer === 'up',
        pcEar,
        pcVoice,
        cloudEar: assumeCloud || feature('asr.own')?.runs === 'cloud' || cloud('asr'),
        cloudVoice: assumeCloud || feature('voice.own')?.runs === 'cloud' || cloud('tts'),
        brain: { name: brain.tone === 'yandex' ? tr.dashboard.alice : brain.value, where },
        computer: data.computer && placement === undefined,
    };
}

/** Кто на деле слышит, думает и говорит сейчас. */
export function truthOf(data: DashData): Solved {
    return solve(scenarioOf(data));
}

/**
 * Узлы орбиты — честно (решение 3): слух у Яндекса в «Шлюзе» — и думает, и говорит Яндекс, хотя выбран DeepSeek;
 * модель на ПК без ПК — Алиса; ПК спит при шлюзе на ПК — никто.
 */
export function nodesOf(data: DashData, truth: Solved = truthOf(data)): Record<NodeKey, NodeView> {
    const nodes = rawNodesOf(data);
    if (truth.silent) {
        const none = (key: NodeKey): NodeView => ({ key, value: tr.dashboard.none, tone: 'off', detail: tr.dashboard.computerSleeps });
        return { brain: none('brain'), think: none('think'), persona: none('persona'), ear: none('ear'), voice: none('voice') };
    }
    if (truth.ear.host === 'yandex' && nodes.ear.tone !== 'yandex') nodes.ear = { key: 'ear', value: tr.dashboard.yandexWord, tone: 'yandex', detail: truth.ear.sub };
    if (truth.brain.host === 'yandex' && nodes.brain.tone !== 'yandex') {
        const brain: NodeView = { key: 'brain', value: tr.dashboard.alice, tone: 'yandex', detail: tr.dashboard.chosenButSilent(truth.brain.sub, nodes.brain.value) };
        nodes.brain = brain;
        nodes.think = thinkNode(data, brain);
        nodes.persona = personaNode(data, brain);
    }
    if (truth.voice.host === 'yandex' && nodes.voice.tone !== 'yandex') nodes.voice = { key: 'voice', value: tr.dashboard.alice, tone: 'yandex', detail: truth.voice.sub };
    return nodes;
}

/** Спутники снизу орбиты — по виду подключения. */
export type SatKey = 'home' | 'mcp' | 'agent' | 'calls' | 'chats';

export const SATELLITES: Array<{ key: SatKey; title: string; kinds: string[] }> = [
    {
        key: 'home',
        get title() {
            return tr.dashboard.satHome;
        },
        kinds: ['smart_home'],
    },
    {
        key: 'mcp',
        get title() {
            return tr.dashboard.satMcp;
        },
        kinds: ['mcp'],
    },
    {
        key: 'agent',
        get title() {
            return tr.dashboard.satAgent;
        },
        kinds: ['agent'],
    },
    {
        key: 'calls',
        get title() {
            return tr.dashboard.satCalls;
        },
        kinds: ['calls'],
    },
    {
        key: 'chats',
        get title() {
            return tr.dashboard.satChats;
        },
        kinds: ['messenger'],
    },
];

/** Строка вида без единого подключения: MCP-серверы добавляют в «Дом» → «Коннекторы», помощников шлюз ещё не умеет. */
export const STUBS: Partial<Record<SatKey, { sub: string; pill: string }>> = {
    mcp: {
        get sub() {
            return tr.dashboard.stubMcp;
        },
        get pill() {
            return tr.dashboard.stubMcpPill;
        },
    },
    agent: {
        get sub() {
            return tr.dashboard.stubAgent;
        },
        get pill() {
            return tr.dashboard.stubAgentPill;
        },
    },
};

export interface SatView {
    key: SatKey;
    title: string;
    tone: Tone;
    /** Настроенные и работающие. */
    on: ConnectorView[];
    /** Все этого вида, выключенные тоже. */
    all: ConnectorView[];
    stub: boolean;
}


/** В «Яндексе» вопрос до нас не доходит — подключения спят. */
export function satellitesOf(data: DashData): SatView[] {
    const sleeping = data.mode === 'relay';
    return SATELLITES.map(({ key, title, kinds }) => {
        const all = (data.connectors ?? []).filter((entry) => kinds.includes(entry.manifest.kind));
        const on = all.filter(working);
        const tone: Tone =
            sleeping || !on.length ? 'off' : on.some((entry) => entry.manifest.cloud) ? 'cloud' : key === 'agent' ? 'agent' : 'own';
        return { key, title, tone, on, all, stub: !all.length && key in STUBS };
    });
}

/** Что из вопроса уходит из дома: слух, мозг, голос — по их цвету. */
export function outsideOf(nodes: Record<NodeKey, NodeView>): string[] {
    const outside = (tone: Tone): boolean => tone === 'cloud' || tone === 'yandex';
    const out: string[] = [];
    if (outside(nodes.ear.tone)) out.push(tr.dashboard.outsideEar);
    if (outside(nodes.brain.tone)) out.push(nodes.brain.value);
    if (outside(nodes.voice.tone)) out.push(tr.dashboard.outsideVoice);
    return out;
}

/** «слух, Claude и голос». */
export function joinRu(items: string[]): string {
    return items.length < 2 ? items.join('') : formatList(items);
}

/**
 * Подпись под колонкой на каждом шаге пути. Пример пути, а не живой запрос: шапка показывает настоящее состояние. В
 * режиме «Микрофон» (`computer`) в покое она зовёт к кнопке: там и начинается разговор.
 */
export function captionOf(stage: TripStage, nodes: Record<NodeKey, NodeView>, mode: PanelMode, computer = false): [string, string] {
    switch (stage) {
        case 'listen':
            return [tr.dashboard.capListening, tr.dashboard.capEar(nodes.ear.value)];
        case 'think':
            return [
                tr.dashboard.capThinking,
                nodes.brain.tone === 'off'
                    ? tr.dashboard.capBrainOff
                    : nodes.brain.tone === 'yandex'
                      ? tr.dashboard.aliceThinks
                      : `${nodes.brain.value} · ${nodes.think.value.toLowerCase()}`,
            ];
        case 'speak':
            return [tr.dashboard.capSpeaking, nodes.voice.tone === 'off' ? tr.dashboard.capVoiceSilent : tr.dashboard.capSpeaks(nodes.voice.value)];
        case 'done': {
            if (mode === 'relay') return [tr.dashboard.capAllYandex, tr.dashboard.capThisIsHow];
            const out = outsideOf(nodes);
            return [out.length ? tr.dashboard.capOutside(joinRu(out)) : tr.dashboard.capAllHome, tr.dashboard.capThisIsHow];
        }
        default:
            return computer ? [tr.dashboard.capPressTalk, tr.dashboard.capAskSomething] : [tr.dashboard.capPressStation, tr.dashboard.capShowPath];
    }
}

/** Узлы, что горят на шаге пути. */
export const LIT: Record<TripStage, NodeKey[]> = {
    idle: [],
    listen: ['ear'],
    think: ['brain', 'think', 'persona'],
    speak: ['voice'],
    done: [],
};

/** Кольцо колонки: цвет того, кто работает на этом шаге; в покое — кто отвечает при режиме; не в сети — серое. */
export function ringOf(stage: TripStage, nodes: Record<NodeKey, NodeView>, mode: PanelMode, online: boolean): string {
    if (stage === 'listen') return LINE[nodes.ear.tone];
    if (stage === 'think') return LINE[nodes.brain.tone];
    if (stage === 'speak') return LINE[nodes.voice.tone];
    if (!online) return LINE.off;
    return mode === 'relay' ? LINE.yandex : LINE.own;
}
