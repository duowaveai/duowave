import { FunctionComponent } from 'preact';
import { createPortal } from 'preact/compat';
import { useEffect, useId, useLayoutEffect, useMemo, useRef, useState } from 'preact/hooks';

import {
    ApiError,
    BrainPick,
    BrainView,
    CharactersView,
    ConnectorView,
    Health,
    LlmEffort,
    ModelsView,
    PanelMode,
    PersonaInfo,
    RequestView,
    SessionSnapshot,
    api,
    can,
    isTight,
    problemOf,
} from '../api';
import { executorOf, withFirst } from '../executor';
import {
    IconBolt,
    IconChat,
    IconCheck,
    IconChevronRight,
    IconClose,
    IconEar,
    IconGauge,
    IconHome,
    IconLinks,
    IconMcp,
    IconPhone,
    IconRobot,
    IconSend,
    IconSmile,
    IconSparkle,
    IconVoice,
} from '../icons';
import { StationDrawing, StationStage } from '../stations';
import { usePanel } from '../shell/context';
import { useOnComputer } from '../voice-input';
import { micOn, onMic } from '../voice/mic';
import { answering, onAnswering } from '../voice/speak';
import { shade, useReducedMotion } from '../stations/kit';
import { safeColor } from '../ui/media';
import { available, useAvailability } from '../availability';
import { BrandMark } from '../ui/brand';
import { BRANDS, type BrandId } from '../brand-icons';
import { Sheet, SkeletonRows } from '../ui/sheet';
import {
    DashData,
    LINE,
    LIT,
    NODE_LABELS,
    NodeKey,
    NodeView,
    OrbitKey,
    STUBS,
    SatKey,
    SatView,
    THINKING,
    Thinking,
    Tone,
    claudeName,
    TripStage,
    WHERE,
    captionOf,
    nodesOf,
    truthOf,
    ourBrain,
    ringOf,
    satellitesOf,
    shortTitle,
    thinkingOf,
    toneClass,
    voiceName,
} from './dash-model';
import { ART_RATIO, CIRCLE, Point, Scene, sceneOf } from './dash-scene';
import { localAsrLine } from './local-asr';
import type { SettingsTab } from './mind';
import { DONE_MS, liveTripOf } from './trip';
import { MODE_TITLES, Solved, Who } from '../truth';
import { WhoThree } from './placement-map';
import { tr } from '../i18n';
import { devMode } from '../shell/dev';
import { byMode } from '../words';
import { useHelpers } from '../helpers';
import { backupPlace } from './brain-backup';
import { fallLine, turboOf } from './turbo-model';
import { AsleepLine, FallLine, TurboBody, TurboSlot } from './turbo-scene';
import { FoldedNotes, HomeStrip, NOTES_FOLDED } from './home-scene';
import { useDeskFloat } from './desk-canvas';
import { remember, useRecalled } from '../shell/memory';
import { CallWord, aliceOnly, chooseWord, removeWords, useCallWords } from './call-words';

/**
 * Карточка Турбо закрыта крестиком (баг-лист 19.11): на свободном столе она висит поверх, у правого края, и мешала
 * виджетам; закрыли — у колонки справа точка-молния, нажали — карточка снова. Помнит пульт (память пульта у шлюза).
 */
const TURBO_HIDDEN = 'desk-turbo-hidden';

/** How far the stage's glow reaches past its core («Минимал»): what the blur(60px) spread was. */
const GLOW_REACH = 110;

// Дашборд колонки: колонка в центре своей орбиты, полукругом по её верху — слух, мозг и голос, снизу — капсула
// подключений. Путь вопроса идёт по этому полукругу: слух → мозг → голос (стол шлюза в
// docs/design/roles/source/SpaceGateway.dc.html).
// Мышление и характер — не на орбите: в них ведут строки из шитов мозга и голоса. Сцена считается под экран
// (dash-scene.ts): орбита и колонка — по высоте, что остаётся до панели вкладок, подписи — всегда одного размера.
// Под колонкой во всех темах — карточка ответа: кто работает, что делает, волна голоса, пока колонка говорит.
// В «Минимале» (look: 'minimal') за колонкой ещё свечение цветом того, кто отвечает, персонаж вылетает из голоса в
// колонку, а при смене колонки новая модель выезжает слева (DESIGN.md, «Минимал» → Motion; ui/minimal.css).

/** Ширина узла: мозг — посередине сверху, слух и голос — по бокам. */
const NODE_W: Record<OrbitKey, number> = { brain: 120, ear: 98, voice: 98 };
/** У слуха и голоса круг сверху, подпись под ним; у мозга — подпись над кругом. */
const BELOW: ReadonlySet<OrbitKey> = new Set(['ear', 'voice']);
const ORDER: OrbitKey[] = ['ear', 'brain', 'voice'];

type Icon = FunctionComponent<{ size?: number }>;
const NODE_ICONS: Record<NodeKey, Icon> = { brain: IconSparkle, think: IconGauge, persona: IconSmile, ear: IconEar, voice: IconVoice };
const SAT_ICONS: Record<SatKey, Icon> = { home: IconHome, mcp: IconMcp, agent: IconRobot, calls: IconPhone, chats: IconSend };

/** Шаги пути вопроса после нажатия на колонку, мс от нажатия. */
const TRIP: Array<[number, TripStage]> = [
    [60, 'listen'],
    [1460, 'think'],
    [3260, 'speak'],
    [5560, 'done'],
    [9560, 'idle'],
];

/** Где нажимается рисунок, который сам этого не говорит: корпус Лайт 2 в рамке 240×200. */
const LITE2_HIT = { x: 17, y: 17, w: 206, h: 176, r: 48 };

/** Сколько ждать между перечитываниями моделей, характера и подключений: они меняются редко. */
const REFRESH_MS = 15_000;
/** Смена колонки в «Минимале»: столько уходит прежняя модель (С6: 1,2 с). */
const SWAP_MS = 1200;

/** Шар персонажа из его цвета: блик сверху слева, тень снизу справа — как шары персонажей на сайте. */
function orbOf(color: string): string {
    return `radial-gradient(circle at 30% 28%, ${shade(color, 0.7)} 0, transparent 42%), radial-gradient(circle at 72% 70%, ${shade(color, -0.35)} 0, transparent 55%), ${color}`;
}

/** Колонка, которая сейчас на сцене, со всем, чем её рисовали: уходящую рисуем теми же входами. */
interface ArtLayer {
    key: string;
    drawing: StationDrawing;
    ring: string;
    stage: StationStage;
    online: boolean;
    timeZone?: string;
}

interface Props {
    health: Health | null;
    /** Сессия выбранной колонки (живая или последняя): её часовой пояс — часы на корпусе. */
    session?: SessionSnapshot;
    /** Чем и как её нарисовать: модель, цвет, часы — из её вида (окно «Колонки»). */
    drawing: StationDrawing;
    online: boolean;
    /** Настоящий запрос этой колонки из живого канала: идущий или только что законченный. */
    trip?: RequestView;
    /** Часы шлюза минус наши: итог запроса держится по шлюзу. */
    skew?: number;
    /** Растёт, когда шлюз сообщил о перемене (голос, характер, модели): дашборд перечитывает своё сразу. */
    version?: number;
    onOpenSettings: (tab: SettingsTab) => void;
    /** В «Дом» → «Коннекторы», сразу в лист этого подключения, если оно названо. */
    onOpenHouse: (connector?: string) => void;
    /** Строка «Слышит · Думает · Говорит» раскрыта: по умолчанию скрыта, открывается словом в конце статуса. */
    whoOpen: boolean;
    onToggleWho: () => void;
    /**
     * Высота сцены, если она своя (стол: колонка в середине поля): без неё сцена занимает то, что остаётся до панели,
     * прижатой к низу экрана.
     */
    room?: number;
    /** В «Дом»: блок Турбо и пустое место для него ведут туда. */
    onOpenHome?: () => void;
    /** Страница Турбо-компьютера, где ставятся его голоса: карточка Турбо без голоса ведёт туда (п. 31). */
    onOpenTurbo?: (helperId: string) => void;
    /**
     * Подписи под колонкой (Алиса, «Приложения», цвета, «Если Турбо уснёт») свёрнуты в значок «Поток запросов» (баг-лист
     * 19.11): на свободном столе, когда на нём свои виджеты, — им нужно место. Нажали — раскрываются поверх стола.
     */
    notesFolded?: boolean;
}

let probe: HTMLDivElement | null = null;

/** Высота экрана при видимой панели браузера (100svh): сцена не прыгает, когда панель прячется при прокрутке. */
function smallViewport(): number {
    if (!probe) {
        probe = document.createElement('div');
        probe.setAttribute('aria-hidden', 'true');
        probe.style.cssText = 'position: fixed; top: 0; left: 0; width: 0; height: 100vh; height: 100svh; visibility: hidden; pointer-events: none;';
        document.body.appendChild(probe);
    }
    return Math.min(window.innerHeight, probe.offsetHeight || window.innerHeight);
}

/**
 * Сцена под экран: ширина — от коробки, высота — сколько остаётся до панели вкладок, пока страница вверху; на столе —
 * своя высота (`fixed`).
 */
function useScene(fixed?: number): [preact.RefObject<HTMLDivElement>, Scene] {
    const box = useRef<HTMLDivElement>(null);
    const [size, setSize] = useState({ width: 390, room: Number.POSITIVE_INFINITY });
    const measure = (): void => {
        const node = box.current;
        if (!node) return;
        const width = node.clientWidth || 390;
        const top = node.getBoundingClientRect().top + window.scrollY;
        const screen = smallViewport();
        // Панель, прижатая к низу экрана (оболочка помечает её data-dock), — сцена кончается над ней.
        const bar = document.querySelector('[data-dock="bottom"]');
        let bottom = screen - 16;
        if (bar) {
            const rect = bar.getBoundingClientRect();
            bottom = screen - (window.innerHeight - rect.bottom) - rect.height - 12;
        }
        const room = fixed ?? Math.round(bottom - top);
        setSize((old) => (old.width === width && old.room === room ? old : { width, room }));
    };
    // После каждой отрисовки: над сценой мог появиться баннер настройки или перенестись пояснение режима.
    useLayoutEffect(measure);
    useLayoutEffect(() => {
        window.addEventListener('resize', measure);
        const observer = typeof ResizeObserver === 'undefined' ? null : new ResizeObserver(() => measure());
        if (box.current) observer?.observe(box.current);
        return () => {
            window.removeEventListener('resize', measure);
            observer?.disconnect();
        };
    }, []);
    return [box, useMemo(() => sceneOf(size.width, size.room), [size.width, size.room])];
}

/**
 * Что пульт делает сам в режиме «Микрофон»: слушает (микрофон открыт) или говорит ответ. Запрос шлюза начинается, когда
 * фраза уже распознана, и кончается, когда «Мозг» дописал, — а пульт ещё говорит: эти шаги орбите показывает он.
 */
function useTalkStage(): 'listen' | 'speak' | null {
    const [listening, setListening] = useState(micOn);
    const [speaking, setSpeaking] = useState(answering);
    useEffect(() => onMic(setListening), []);
    useEffect(() => onAnswering(setSpeaking), []);
    return listening ? 'listen' : speaking ? 'speak' : null;
}

export function Dashboard({ health, session, drawing, online, trip, skew = 0, version = 0, onOpenSettings, onOpenHouse, whoOpen, onToggleWho, room, onOpenHome, onOpenTurbo, notesFolded }: Props) {
    const mode: PanelMode = health?.panelMode ?? (health?.mode === 'relay' ? 'relay' : 'gateway');
    /** Свободный стол: карточка Турбо висит над ним, а не стоит рядом с колонкой. */
    const floatHost = useDeskFloat();
    const turboHidden = useRecalled<boolean>(TURBO_HIDDEN) === true;
    const computer = useOnComputer();
    // Турбо и «где сейчас»: кто на деле слышит, думает и говорит, и второе тело сцены (решение владельца 16.11).
    const helpers = useHelpers();
    const talkStage = useTalkStage();
    const [models, setModels] = useState<ModelsView | null>(null);
    const [modelsFailed, setModelsFailed] = useState(false);
    const [modelsAbsent, setModelsAbsent] = useState(false);
    // На колонке слух и голос — по списку шлюза: у компьютера, в облаке, у Яндекса.
    const availability = useAvailability();
    const [persona, setPersona] = useState<PersonaInfo | null>(null);
    const [connectors, setConnectors] = useState<ConnectorView[] | null>(null);
    const [brainView, setBrainView] = useState<BrainView | null>(null);
    const [characters, setCharacters] = useState<CharactersView | null>(null);
    const [stage, setStage] = useState<TripStage>('idle');
    const [sheet, setSheet] = useState<NodeKey | 'links' | 'words' | null>(null);
    // Слова-активаторы у «Слышит» (замечание 11 теста 19.11): выбрать слово, «Только Алиса», убрать все — как молния
    // Турбо у «Говорит». Только у колонки: у компьютера в режиме «Микрофон» своё слово «ДиВи».
    const callWords = useCallWords();
    const panel = usePanel();
    const wordsShown = !computer && can('plugins.manage');
    const wordOn = callWords.words?.find((word) => word.enabled);
    const [, setTick] = useState(0);
    // Материал один — «Минимал» (docs/plans/desk.md): свечение, полёт персонажа, выезд новой модели — всегда.
    const minimal = true;
    const reduced = useReducedMotion();
    const [box, scene] = useScene(room);
    // Персонажи нужны только сцене «Минимала»: цвет того, кто влетает в колонку.
    const wantCharacters = useRef(minimal);
    wantCharacters.current = minimal;
    const blur = `dash-blur-${useId().replace(/[^A-Za-z0-9_-]/g, '')}`;
    const timers = useRef<number[]>([]);
    const alive = useRef(true);
    const loading = useRef(false);
    const again = useRef(false);

    /** Модели, характер и подключения: по своему праву каждое; ошибка оставляет прежнее. После действия — ещё раз. */
    const reload = async (): Promise<void> => {
        if (loading.current) {
            again.current = true;
            return;
        }
        loading.current = true;
        try {
            const [freshModels, freshPersona, freshConnectors, freshBrain, freshCharacters] = await Promise.all([
                // Не дольше 10 с: на колонке через проход компьютера запрос, бывало, висел — окно «Говорит» держало
                // заглушки вечно (проверка начисто 18.11, п. 27). Не дождались — «не прочитались», как при ошибке.
                can('station.view')
                    ? within(api.models(), MODELS_WAIT_MS).catch((e: unknown) => (e instanceof ApiError && e.status === 501 ? ABSENT : undefined))
                    : null,
                can('station.view') ? within(api.persona(), MODELS_WAIT_MS).catch(() => null) : null,
                can('connectors.view')
                    ? within(api.connectors(), MODELS_WAIT_MS)
                          .then((list) => list.connectors)
                          .catch(() => null)
                    : null,
                can('connectors.view') ? within(api.brain(), MODELS_WAIT_MS).catch(() => null) : null,
                wantCharacters.current && can('station.view') ? within(api.characters(), MODELS_WAIT_MS).catch(() => null) : null,
            ]);
            if (!alive.current) return;
            if (freshModels && freshModels !== ABSENT) setModels(freshModels);
            setModelsFailed(freshModels === undefined);
            setModelsAbsent(freshModels === ABSENT);
            if (freshPersona) setPersona(freshPersona);
            if (freshConnectors) setConnectors(freshConnectors);
            if (freshBrain) setBrainView(freshBrain);
            if (freshCharacters) setCharacters(freshCharacters);
        } finally {
            loading.current = false;
            if (again.current && alive.current) {
                again.current = false;
                void reload();
            }
        }
    };

    useEffect(() => {
        alive.current = true;
        void reload();
        const id = window.setInterval(() => void reload(), REFRESH_MS);
        return () => {
            alive.current = false;
            window.clearInterval(id);
            timers.current.forEach((timer) => window.clearTimeout(timer));
        };
    }, []);

    // Перемена на шлюзе (голос, характер, модели сменили с другого телефона): перечитать, не дожидаясь опроса.
    useEffect(() => {
        if (version) void reload();
    }, [version]);

    // Тему переключили на «Минимал» — сцене нужен цвет персонажа сразу, а не через опрос.
    useEffect(() => {
        if (minimal && !characters) void reload();
    }, [minimal]);

    // Настоящий запрос важнее примера: шаги примера останавливаются.
    useEffect(() => {
        if (!trip || trip.status !== 'active') return;
        timers.current.forEach((timer) => window.clearTimeout(timer));
        timers.current = [];
        setStage('idle');
    }, [trip?.id]);

    // Итог висит DONE_MS, потом орбита в покое: ещё одна отрисовка, когда срок выйдет.
    useEffect(() => {
        if (!trip || trip.status === 'active') return;
        const left = Date.parse(trip.startedAt) + (trip.ms ?? 0) + DONE_MS - (Date.now() + skew);
        if (left <= 0) return;
        const timer = window.setTimeout(() => setTick((tick) => tick + 1), left + 50);
        return () => window.clearTimeout(timer);
    }, [trip?.id, trip?.status, skew]);

    const data: DashData = {
        mode,
        routes: health?.skills?.routes,
        brain: brainView?.status ?? health?.brain,
        brainView,
        models,
        modelsFailed,
        modelsAbsent,
        persona,
        connectors,
        availability,
        computer,
        yandexEar: health?.yandexEar,
        helpers: helpers.view,
    };
    const truth = truthOf(data);
    const nodes = nodesOf(data, truth);
    const sats = satellitesOf(data);
    const tripNow = liveTripOf(trip, Date.now() + skew, nodes);
    // Режим «Микрофон»: пока пульт слушает или ещё говорит ответ, орбита показывает это, а не итог запроса.
    const own: TripStage | null =
        !computer || !talkStage ? null : talkStage === 'listen' ? 'listen' : !tripNow || tripNow.stage === 'done' ? 'speak' : null;
    const live = own ? undefined : tripNow;
    const shown = own ?? live?.stage ?? stage;
    const lit = own ? LIT[own] : (live?.lit ?? LIT[stage]);
    const [caption, captionSub] = live?.caption ?? captionOf(own ?? stage, nodes, mode, computer);
    // Кто работает на шаге настоящего запроса — тот и цвет; в примере — цвет узла.
    const brainTone = live?.tone ?? nodes.brain.tone;
    const voiceTone = live?.tone ?? nodes.voice.tone;
    const baseRing = live?.tone && live.stage !== 'done' ? LINE[live.tone] : ringOf(shown, nodes, mode, online);
    // «Минимал»: персонаж со своим цветом отвечает — его шар влетает в колонку, и колонка красится под него.
    const activeCharacter = characters?.active?.id ? characters.characters.find((entry) => entry.id === characters.active?.id) : undefined;
    const characterColor = minimal && nodes.persona.tone === 'own' ? safeColor(activeCharacter?.color) : undefined;
    const orb = characterColor ? orbOf(characterColor) : undefined;
    const inside = !!characterColor && shown === 'speak';
    const ring = inside && characterColor ? characterColor : baseRing;
    const working = shown === 'listen' || shown === 'think' || shown === 'speak';
    const glow = working ? ring : 'var(--glow-rest)';
    /** Кто работает на этом шаге — точка карточки ответа и цвет луча. */
    const stepTone: Tone =
        shown === 'listen'
            ? (live?.tone ?? nodes.ear.tone)
            : shown === 'think'
              ? brainTone
              : shown === 'speak'
                ? voiceTone
                : !online
                  ? 'off'
                  : mode === 'relay'
                    ? 'yandex'
                    : 'own';
    const { Art } = drawing;
    const sleeping = mode === 'relay';
    const linked = sats.flatMap((sat) => sat.on);
    const mayLinks = can('connectors.view');
    const linksCaption = sleeping
        ? tr.dashboard.linksSleeping
        : !mayLinks
          ? tr.dashboard.links
          : connectors
            ? tr.dashboard.linksCount(linked.length)
            : tr.dashboard.linksReading;

    /** Один вопрос по орбите: слух слышит, мозг думает, голос отвечает — и в конце видно, что ушло из дома. */
    const play = (): void => {
        timers.current.forEach((timer) => window.clearTimeout(timer));
        setStage('idle');
        timers.current = TRIP.map(([ms, next]) => window.setTimeout(() => setStage(next), ms));
    };

    const context: SpecContext = { data, nodes, health, openSettings: onOpenSettings, openSheet: setSheet };
    const { art } = scene;
    /** Рисунок колонки — 240×200 точек; нажимается её корпус — где он, говорит рисунок (у ДиВи — его тело). */
    const k = art.size / 240;
    const hit = drawing.hit ?? LITE2_HIT;
    const artStage = (shown === 'done' ? 'idle' : shown) as StationStage;

    // Смена колонки: прежняя уходит вправо, новая выезжает слева из глубины (только «Минимал» и без «меньше движения»).
    const stationKey = session?.deviceTag ?? drawing.title;
    const current: ArtLayer = { key: stationKey, drawing, ring, stage: artStage, online, timeZone: session?.device?.timezone };
    const shownArt = useRef<ArtLayer>(current);
    const [leaving, setLeaving] = useState<ArtLayer | null>(null);
    const [entering, setEntering] = useState<string | null>(null);
    const swapTimer = useRef(0);
    useEffect(() => {
        const before = shownArt.current;
        shownArt.current = current;
        if (before.key === stationKey || !minimal || reduced) return;
        window.clearTimeout(swapTimer.current);
        setLeaving(before);
        setEntering(stationKey);
        swapTimer.current = window.setTimeout(() => setLeaving(null), SWAP_MS);
    });
    useEffect(() => () => window.clearTimeout(swapTimer.current), []);
    const artBox = { left: `${art.left}px`, top: `${art.top}px`, width: `${art.size}px`, height: `${art.size * ART_RATIO}px` };

    // Второе тело: Турбо рядом со шлюзом; у колонки без Турбо — пустое место для него; у одинокого компьютера без
    // встроенного Турбо — ничего (стартовый стол остаётся простым). Под сценой — что встанет вместо Турбо, если уснёт.
    const station = health?.placement?.role === 'station';
    const turbo = turboOf(helpers.view);
    const openHome = onOpenHome ?? (() => undefined);
    // Турбо, у которого голос не готов: карточка ведёт прямо на его страницу, где голос ставится (проверка начисто
    // 18.11, п. 31: «Говорит — не готово», а куда идти — не видно).
    const voiceMissing = !!turbo && !turbo.builtin && turbo.abilities.some((ability) => ability.id === 'voice' && !ability.ready);
    const openTurbo = turbo && voiceMissing && onOpenTurbo ? () => onOpenTurbo(turbo.helper.id) : openHome;
    // Под сценой (узкое окно, столбец) у блока свой крестик; над свободным столом крестик у его рамки (ниже).
    const sideOf = (onHide?: () => void) =>
        turbo ? <TurboBody turbo={turbo} onOpen={openTurbo} onHide={onHide} /> : station && helpers.view ? <TurboSlot onAdd={openHome} onHide={onHide} /> : null;
    const side = sideOf();
    const cloudOn = (id: string): boolean =>
        connectors?.some((entry) => entry.manifest.id === id && entry.manifest.cloud && (entry.status.state === 'ready' || entry.status.state === 'configured')) ?? false;
    const backup = brainView ? backupPlace(brainView) : null;
    const away = helpers.view?.away ?? 'home';
    const fall = fallLine(turbo, {
        station,
        computer,
        mode,
        away,
        cloudEar: cloudOn('asr'),
        cloudVoice: cloudOn('tts'),
        backup: !!backup && backup.helper !== turbo?.helper.id && !(backup.cloud && away !== 'any'),
    });
    const asleep = turbo && !turbo.builtin && !turbo.up && turbo.helper.allowed !== false;
    // На свободном столе второе тело не стоит рядом с колонкой, а висит поверх стола у правого края: места у виджетов
    // не берёт, закрывается крестиком; закрыто — у колонки точка-молния.
    const floating = !!floatHost && !!side;
    /** Блок Турбо стоит у колонки: в узком окне, и его не скрыли. */
    const inline = !floating && !!side && !turboHidden;
    // «Поток запросов» (полоса дома и «Если Турбо уснёт») сворачивается крестиком в любом окне — и в узком, где сцена
    // стоит в ленте (замечание 16 после 0.18.1: крестик был только на свободном столе).
    const fold = !!notesFolded;
    const notes = (folded: boolean) => (
        <>
            <HomeStrip alice={station && mode !== 'own'} clouds={cloudOn('asr') || cloudOn('tts')} onFold={!folded ? () => remember(NOTES_FOLDED, true) : undefined} />
            {fall && <FallLine items={fall.items} modeFalls={fall.modeFalls} backup={backup ?? undefined} />}
        </>
    );

    return (
        <>
            {asleep && <AsleepLine turbo={turbo} station={station} />}
            <TruthStrip nodes={nodes} truth={truth} mode={mode} open={whoOpen} onToggle={onToggleWho} />
            <div class={`dash-pair${inline ? ' has-side' : ''}`}>
            <div ref={box} class="dash-stage" style={{ height: `${Math.round(scene.height * scene.scale)}px` }}>
                <div
                    class="dash-board"
                    style={{
                        left: `calc(50% - ${scene.width / 2}px)`,
                        width: `${scene.width}px`,
                        height: `${scene.height}px`,
                        transform: scene.scale < 1 ? `scale(${scene.scale})` : undefined,
                    }}
                >
                    {minimal && (
                        // The glow is a radial gradient that fades to nothing at its own edge, not a blurred box: WebKit
                        // (the desktop app's window) cuts a filter's blur at the scaled board's and the scroll area's
                        // edges, and the glow showed straight corners there. GLOW_REACH stands for the blur's spread.
                        <div
                            class="dash-glow"
                            aria-hidden="true"
                            style={{
                                left: `${art.left + art.size / 2 - art.size * 0.86 - GLOW_REACH}px`,
                                top: `${art.top + art.size * ART_RATIO * 0.56 - art.size * 0.6 - GLOW_REACH}px`,
                                width: `${art.size * 1.72 + 2 * GLOW_REACH}px`,
                                height: `${art.size * 1.2 + 2 * GLOW_REACH}px`,
                                '--glow': glow,
                            }}
                        />
                    )}
                    <svg class="dash-orbit" aria-hidden="true" width={scene.width} height={scene.height} viewBox={`0 0 ${scene.width} ${scene.height}`}>
                        {/* Орбита во всех темах: искра бежит по кругу, на шаге вопроса комета летит от слуха к мозгу и от
                            мозга к голосу цветом того, кто работает. «Минимал» рисует её тоньше (ui/minimal.css). */}
                        <defs>
                            <filter id={blur} x="-20%" y="-20%" width="140%" height="140%">
                                <feGaussianBlur stdDeviation="3" />
                            </filter>
                        </defs>
                        <path class="dash-track" d={scene.track} />
                        <path class="dash-spark dash-anim" d={scene.track} pathLength={100} stroke-dasharray="8 92" />
                        {shown === 'think' && <Comet key={`think-${trip?.id ?? 'demo'}`} d={scene.toBrain} tone={brainTone} blur={blur} />}
                        {shown === 'speak' && <Comet key={`speak-${trip?.id ?? 'demo'}`} d={scene.toVoice} tone={voiceTone} blur={blur} />}
                    </svg>

                    {leaving && leaving.key !== stationKey && (
                        <div key={`leave-${leaving.key}`} class="dash-art leave" style={artBox} aria-hidden="true">
                            <leaving.drawing.Art
                                skin={leaving.drawing.color}
                                clock={leaving.drawing.clock}
                                sticker={leaving.drawing.sticker}
                                ring={leaving.ring}
                                stage={leaving.stage}
                                offline={!leaving.online}
                                timeZone={leaving.timeZone}
                            />
                        </div>
                    )}
                    <div key={`art-${stationKey}`} class={`dash-art${minimal && entering === stationKey ? ' enter' : ''}`} style={artBox}>
                        <Art
                            skin={drawing.color}
                            clock={drawing.clock}
                            sticker={drawing.sticker}
                            ring={ring}
                            stage={artStage}
                            offline={!online}
                            timeZone={session?.device?.timezone}
                        />
                    </div>
                    <button
                        class="dash-speaker"
                        style={{
                            left: `${art.left + hit.x * k}px`,
                            top: `${art.top + hit.y * k}px`,
                            width: `${hit.w * k}px`,
                            height: `${hit.h * k}px`,
                            borderRadius: `${hit.r * k}px`,
                        }}
                        aria-label={live ? tr.dashboard.showTripExample(live.label) : tr.dashboard.showTrip}
                        onClick={play}
                    />
                    {/* Карточка ответа во всех темах: точка — кто работает, что делает и что услышал или ответил; пока
                        колонка говорит — волна голоса. */}
                    <div
                        class={`dash-caption ${toneClass(stepTone)}${live ? ' live' : ''}`}
                        style={{ left: `${(scene.width - scene.caption.width) / 2}px`, top: `${scene.caption.top}px`, width: `${scene.caption.width}px` }}
                        aria-live="polite"
                    >
                        <span class="dash-caption-head">
                            <b>{caption}</b>
                            {shown === 'speak' && (
                                <span class="dash-wave" aria-hidden="true" style={inside && characterColor ? { '--wave': characterColor } : undefined}>
                                    <i />
                                    <i />
                                    <i />
                                    <i />
                                    <i />
                                </span>
                            )}
                        </span>
                        <span class="dash-caption-sub">{captionSub}</span>
                    </div>

                    {/* «Подключено» — когда подключено хоть что-то: «Подключено · 0» и пять серых значков у новичка — шум
                        (владелец, 16.11). Подключают в «Приложениях» → «Подключения». */}
                    {linked.length > 0 && (
                        <button
                            class="dash-links"
                            style={{ left: `${scene.width / 2}px`, top: `${scene.linksTop}px` }}
                            aria-label={`${
                                sleeping
                                    ? tr.dashboard.linksSleepingRelay
                                    : connectors
                                      ? tr.dashboard.linksLinked(linked.length, linked.map((entry) => entry.manifest.title).join(', '))
                                      : tr.dashboard.links
                            }. ${tr.dashboard.open}`}
                            onClick={() => setSheet('links')}
                        >
                            <span class="dash-sats" aria-hidden="true">
                                {sats.map((sat) => (
                                    <Satellite key={sat.key} sat={sat} sleeping={sleeping} />
                                ))}
                            </span>
                            <span class={sleeping ? 'c-sub' : 'c-text'}>{linksCaption}</span>
                        </button>
                    )}

                    {orb && !reduced && (
                        // Шар персонажа: в покое спрятан под кругом голоса, на ответе вылетает из него в кольцо колонки
                        // и гаснет.
                        <span
                            class={`dash-fly${inside ? ' inside' : ''}`}
                            aria-hidden="true"
                            style={{
                                left: `${scene.nodes.voice.x}px`,
                                top: `${scene.nodes.voice.y}px`,
                                '--fly-x': `${scene.ring.x - scene.nodes.voice.x}px`,
                                '--fly-y': `${scene.ring.y - scene.nodes.voice.y}px`,
                            }}
                        >
                            <span class="dash-fly-orb" style={{ '--g': orb }}>
                                <i />
                            </span>
                        </span>
                    )}

                    {ORDER.map((key) => (
                        <OrbitNode
                            key={key}
                            spot={key}
                            node={nodes[key]}
                            at={scene.nodes[key]}
                            lit={lit.includes(key)}
                            onOpen={() => setSheet(key)}
                        />
                    ))}
                    {wordsShown && (
                        <button
                            class={`dash-word-dot${wordOn ? '' : ' is-off'}`}
                            style={{ left: `${scene.nodes.ear.x - CIRCLE / 2 - 42}px`, top: `${scene.nodes.ear.y - 14}px` }}
                            aria-label={tr.dashboard.wordsOpen(wordOn?.word ?? tr.wizard.alice)}
                            title={tr.dashboard.wordsOpen(wordOn?.word ?? tr.wizard.alice)}
                            onClick={() => setSheet('words')}
                        >
                            <IconChat size={14} />
                        </button>
                    )}
                    {!!side && turboHidden && (
                        <button
                            class={`dash-turbo-dot${turbo ? (turbo.up || turbo.builtin ? '' : ' is-off') : ' is-empty'}`}
                            style={{ left: `${scene.nodes.voice.x + CIRCLE / 2 + 14}px`, top: `${scene.nodes.voice.y - 14}px` }}
                            aria-label={tr.desk.turboShow}
                            title={tr.desk.turboShow}
                            onClick={() => remember(TURBO_HIDDEN, undefined)}
                        >
                            <IconBolt size={14} />
                        </button>
                    )}
                </div>
            </div>
            {inline && sideOf(() => remember(TURBO_HIDDEN, true))}
            </div>
            {floating &&
                !turboHidden &&
                floatHost &&
                createPortal(
                    <div class="desk-float-card">
                        <button class="desk-float-close" aria-label={tr.desk.turboHide} title={tr.desk.turboHide} onClick={() => remember(TURBO_HIDDEN, true)}>
                            <IconClose size={14} />
                        </button>
                        {side}
                    </div>,
                    floatHost,
                )}
            {/* Дом целиком (docs/plans/home-and-devices.md, этап 1): колонки, что слышат для центра, Алиса и облака,
                приложения, легенда цветов; под ними — что встанет вместо Турбо, если он уснёт. Свёрнуто — значок. */}
            {fold ? <FoldedNotes>{notes(true)}</FoldedNotes> : notes(false)}

            {sheet === 'words' && (
                <ChoiceSheet
                    key="words"
                    spec={wordsSpec(callWords.words, callWords.station, () => panel.go('magicplace', 'store', undefined, { kind: 'word' }))}
                    onClose={() => setSheet(null)}
                    onApplied={callWords.reload}
                />
            )}
            {sheet && sheet !== 'links' && sheet !== 'words' && (
                <ChoiceSheet key={sheet} spec={specFor(sheet, context)} onClose={() => setSheet(null)} onApplied={() => void reload()} />
            )}
            {sheet === 'links' && (
                <LinksSheet
                    sats={sats}
                    sleeping={sleeping}
                    loaded={!!connectors}
                    allowed={mayLinks}
                    onClose={() => setSheet(null)}
                    onOpenHouse={onOpenHouse}
                />
            )}
        </>
    );
}

function Comet({ d, tone, blur }: { d: string; tone: Tone; blur: string }) {
    return (
        <g class={toneClass(tone)}>
            <path
                class="dash-comet dash-anim"
                d={d}
                stroke-width="9"
                opacity="0.5"
                filter={`url(#${blur})`}
                pathLength={100}
                stroke-dasharray="14 100"
                stroke-dashoffset="14"
            />
            <path class="dash-comet dash-anim" d={d} stroke-width="3" pathLength={100} stroke-dasharray="14 100" stroke-dashoffset="14" />
        </g>
    );
}

function OrbitNode({ spot, node, at, lit, onOpen }: { spot: OrbitKey; node: NodeView; at: Point; lit: boolean; onOpen: () => void }) {
    const width = NODE_W[spot];
    const below = BELOW.has(spot);
    const NodeIcon = NODE_ICONS[spot];
    const where = node.tone !== 'off' ? `, ${WHERE[node.tone]}` : '';
    const label = (
        <span class="dash-label">
            <small>{NODE_LABELS[spot]}</small>
            <b>{node.value}</b>
        </span>
    );
    const circle = (
        <span class={`dash-circle ${toneClass(node.tone)}${lit ? ' on' : ''}`}>
            <NodeIcon size={22} />
            {lit && <span class="dash-pulse dash-anim" />}
        </span>
    );
    return (
        <button
            class="dash-node"
            // Узел по имени: комета агента вылетает от «Мозга» (ui/comet.ts).
            data-node={spot}
            // Круг узла — ровно на орбите: у подписи сверху узел отсчитывается от низа круга.
            style={{
                left: `${at.x - width / 2}px`,
                top: `${below ? at.y - CIRCLE / 2 : at.y + CIRCLE / 2}px`,
                width: `${width}px`,
                transform: below ? undefined : 'translateY(-100%)',
            }}
            aria-label={tr.dashboard.nodeAria(NODE_LABELS[spot], node.value, where)}
            onClick={onOpen}
        >
            {below ? (
                <>
                    {circle}
                    {label}
                </>
            ) : (
                <>
                    {label}
                    {circle}
                </>
            )}
        </button>
    );
}

/** Спутник в капсуле подключений: плоский круг цвета исполнителя, у нескольких одного вида — счётчик. */
function Satellite({ sat, sleeping }: { sat: SatView; sleeping: boolean }) {
    const SatIcon = SAT_ICONS[sat.key];
    return (
        <span class={`dash-sat ${toneClass(sat.tone)}`}>
            <SatIcon size={14} />
            {!sleeping && sat.on.length > 1 && <span class="dash-badge">{sat.on.length}</span>}
        </span>
    );
}

// ─── Шиты узлов ──────────────────────────────────────────────────────────

/**
 * Шит слов-активаторов (замечание 11 теста 19.11): слова, что стоят, — слушает одно; «Только Алиса» — все выключены
 * (ОЗУ колонки свободна, файлы остаются); «Убрать все слова» — стереть с колонки, с вопросом. Нет слов — в магазин.
 */
function wordsSpec(words: CallWord[] | null, station: string | undefined, find: () => void): Spec {
    const t = tr.dashboard;
    const list = words ?? [];
    const on = list.find((word) => word.enabled);
    return {
        key: 'ear',
        tone: 'own',
        title: t.wordsTitle,
        about: t.wordsAbout,
        empty: words === null ? t.wordsReading : undefined,
        choices: [
            ...list.map((word): Choice => ({
                id: word.id,
                name: `«${word.word}»`,
                sub: word.enabled ? t.wordListening : t.wordOff,
                on: word.enabled,
                apply: async () => (await chooseWord(station, list, word.id), t.wordChosen(word.word)),
            })),
            {
                id: 'alice',
                name: t.wordAliceOnly(tr.wizard.alice),
                sub: t.wordAliceOnlySub,
                on: !on,
                apply: async () => (await aliceOnly(station, list), t.wordAliceChosen(tr.wizard.alice)),
            },
            ...(list.length
                ? [
                      {
                          id: 'remove',
                          name: t.wordsRemove,
                          sub: t.wordsRemoveSub,
                          on: false,
                          confirm: t.wordsRemoveConfirm,
                          apply: async () => (await removeWords(station, list), t.wordsRemoved),
                      } satisfies Choice,
                  ]
                : []),
        ],
        notes: [],
        links: [{ label: t.wordsFind, open: find }],
    };
}

/** Строка выбора в шите узла. */
interface Choice {
    id: string;
    name: string;
    sub: string;
    /** Знак сервиса слева (модели провайдеров). */
    brand?: string;
    pill?: { text: string; tone: Tone };
    on: boolean;
    disabled?: boolean;
    /** Сначала «Станет … вместо …»: смена маршрута, уход колонки к Яндексу, долгий перезапуск. */
    confirm?: string;
    /** Применить; ответ — что сказать после. `anyway`: шлюз предупредил «памяти впритык», владелец включает всё равно. */
    apply?: (anyway?: boolean) => Promise<string>;
}

interface Spec {
    key: NodeKey;
    tone: Tone;
    title: string;
    about: string;
    choices: Choice[];
    /** Вместо выбора, пока данных нет. */
    empty?: string;
    /**
     * Вместо выбора, когда данных и не будет: не прочитались или их здесь нет — словами, а не серыми строками (те
     * означают «читаю», проверка начисто 18.11, п. 27).
     */
    instead?: string;
    notes: string[];
    links: Array<{ label: string; open: () => void }>;
}

interface SpecContext {
    data: DashData;
    nodes: Record<NodeKey, NodeView>;
    health: Health | null;
    openSettings: (tab: SettingsTab) => void;
    /** Открыть другой шит вместо этого: мышление — из мозга, характер — из голоса. */
    openSheet: (key: NodeKey) => void;
}

const pillClass = (tone: Tone): string => `pill pill-${tone === 'cloud' ? 'yandex' : tone}`;
const relayAbout = (): string => tr.dashboard.relayAbout;
const adminOnly = (): string => tr.dashboard.adminOnly;

/** Мышление и характер не на орбите: строка в шите мозга или голоса, когда там есть что выбрать (не Алиса, не «нет»). */
function sheetLink(key: 'think' | 'persona', { nodes, openSheet }: SpecContext): Spec['links'] {
    const node = nodes[key];
    if (node.tone === 'yandex' || node.tone === 'off') return [];
    const value = key === 'think' ? node.value.toLowerCase() : node.value;
    return [{ label: `${NODE_LABELS[key]}: ${value}`, open: () => openSheet(key) }];
}

/**
 * Что показать в шите узла: варианты с тем, что можно здесь и сейчас, и честное «где это меняется» для остального. В
 * режиме «Микрофон» вариантов Яндекса нет: ни Алисы мозгом, ни слуха Яндекса, ни голоса Алисы.
 */
function specFor(key: NodeKey, context: SpecContext): Spec {
    const spec = stationSpec(key, context);
    return context.data.computer ? { ...spec, choices: spec.choices.filter((choice) => choice.id !== 'yandex') } : spec;
}

function stationSpec(key: NodeKey, context: SpecContext): Spec {
    switch (key) {
        case 'brain':
            return brainSpec(context);
        case 'think':
            return thinkSpec(context);
        case 'persona':
            return personaSpec(context);
        case 'ear':
            return earSpec(context);
        default:
            return voiceSpec(context);
    }
}

/** Мозг: модели добавленных провайдеров, модель из .env или Алиса — первый шаг маршрута «Разговор». */
function brainSpec(context: SpecContext): Spec {
    const { data, nodes, health, openSettings } = context;
    const relay = data.mode === 'relay';
    const executor = executorOf(data.routes?.chat ?? 'own', data.mode);
    const ours = ourBrain(data.brain);
    const mayRoute = can('routes.edit');
    const mayChoose = can('models.manage');
    const setFirst = async (first: 'own' | 'yandex'): Promise<void> => {
        const current = await api.routes();
        if (executorOf(current.routes.chat ?? 'own', data.mode) === first) return;
        const result = await api.setRoutes({ ...current.routes, chat: withFirst(current.routes.chat, first) });
        if (result.problems.length) throw new Error(result.problems.join('; '));
    };
    const view = data.brainView;
    /**
     * В режиме «Алиса» модели не серые (замечание 3 после 0.18.1: включить свой ИИ можно было только в «Устройства» →
     * колонка → «Комната»): выбор модели спрашивает «Включить «Алиса + ваш ИИ»» и одной кнопкой выбирает её, ставит ИИ
     * первым в «Разговоре» и переключает колонку — она переподключается.
     */
    const mayTogether = relay && mayChoose && can('station.mode') && available('mode.gateway');
    const together = async (pick: BrainPick | null): Promise<string> => {
        await api.chooseBrain({ fast: pick });
        const current = await api.routes();
        if (executorOf(current.routes.chat ?? 'own', 'gateway') !== 'own') {
            const result = await api.setRoutes({ ...current.routes, chat: withFirst(current.routes.chat, 'own') });
            if (result.problems.length) throw new Error(result.problems.join('; '));
        }
        await api.setMode('gateway');
        return tr.dashboard.togetherOn;
    };
    const about = relay ? (mayTogether ? tr.dashboard.relayAboutPick : relayAbout()) : tr.dashboard.brainAbout;
    const alice: Choice = {
        id: 'yandex',
        name: tr.dashboard.alice,
        sub: data.mode === 'own' ? tr.dashboard.notAllowedInOwn : tr.dashboard.yandexAnswers,
        pill: { text: WHERE.yandex, tone: 'yandex' },
        on: executor === 'yandex',
        disabled: relay || data.mode === 'own' || !mayRoute,
        confirm: tr.dashboard.becomesAliceInstead(nodes.brain.value),
        apply: async () => {
            await setFirst('yandex');
            return tr.dashboard.appliedNext;
        },
    };
    if (!view) {
        // Без права смотреть подключения: одна строка «наша модель», как раньше.
        return {
            key: 'brain',
            tone: nodes.brain.tone,
            title: NODE_LABELS.brain,
            about: relay ? relayAbout() : tr.dashboard.brainAbout,
            choices: [
                {
                    id: 'own',
                    name: ours.name,
                    sub: ours.detail,
                    pill: ours.usable ? { text: WHERE[ours.tone], tone: ours.tone } : undefined,
                    on: executor === 'own',
                    disabled: relay || !mayRoute || !ours.usable,
                    confirm: tr.dashboard.becomesInstead(ours.name, nodes.brain.value),
                    apply: async () => {
                        await setFirst('own');
                        return tr.dashboard.appliedNext;
                    },
                },
                alice,
            ],
            notes: [!mayRoute && !relay ? adminOnly() : '', devMode() && health?.dialog ? tr.dashboard.gatewayNote(health.dialog) : ''].filter(Boolean),
            links: sheetLink('think', context),
        };
    }
    // Модели каждого провайдера: выбранные, проверенная, советы пресета — без длинных списков (весь список — в «Мозге»).
    const fast = view.choice.fast;
    const models: Choice[] = view.providers.flatMap((provider) => {
        const preset = view.presets.find((entry) => entry.id === provider.preset);
        const named = [fast, view.choice.smart].filter((pick): pick is BrainPick => pick?.provider === provider.id).map((pick) => pick.model);
        const candidates = [...new Set([...named, provider.check?.model, preset?.suggested?.fast, preset?.suggested?.smart, ...provider.models.slice(0, 2)])]
            .filter((model): model is string => !!model)
            .slice(0, 3);
        const tone: Tone = provider.cloud ? 'cloud' : 'own';
        return candidates.map((model) => {
            const name = provider.kind === 'anthropic' ? claudeName(model) : model;
            const on = executor === 'own' && fast?.provider === provider.id && fast.model === model;
            return {
                id: `${provider.id}:${model}`,
                name,
                sub: `${provider.title}${provider.check && !provider.check.ok ? tr.dashboard.failedCheck : ''}`,
                brand: provider.brand,
                pill: { text: WHERE[tone], tone },
                on,
                disabled: relay ? !mayTogether : !mayChoose || (executor !== 'own' && !mayRoute),
                confirm: relay ? tr.dashboard.becomesTogether(name) : tr.dashboard.becomesInstead(name, nodes.brain.value),
                apply: async () => {
                    if (relay) return together({ provider: provider.id, model });
                    await setFirst('own');
                    await api.chooseBrain({ fast: { provider: provider.id, model } });
                    return tr.dashboard.appliedNext;
                },
            };
        });
    });
    const envChoice: Choice[] = view.env.usable
        ? [
              {
                  id: 'env',
                  name: `${view.env.provider === 'anthropic' ? claudeName(view.env.models.fast) : view.env.models.fast}`,
                  sub: tr.dashboard.fromEnv(view.env.title),
                  pill: { text: WHERE[view.env.cloud ? 'cloud' : 'own'], tone: view.env.cloud ? 'cloud' : 'own' },
                  on: executor === 'own' && !fast,
                  disabled: relay ? !mayTogether : !mayChoose || (executor !== 'own' && !mayRoute),
                  confirm: relay ? tr.dashboard.becomesTogether(view.env.models.fast) : tr.dashboard.becomesEnv(nodes.brain.value),
                  apply: async () => {
                      if (relay) return together(null);
                      await setFirst('own');
                      await api.chooseBrain({ fast: null });
                      return tr.dashboard.appliedNext;
                  },
              },
          ]
        : [];
    return {
        key: 'brain',
        tone: nodes.brain.tone,
        title: NODE_LABELS.brain,
        about,
        choices: [...models, ...envChoice, alice],
        notes: [
            !mayChoose && !relay ? adminOnly() : '',
            models.length ? '' : tr.dashboard.noProviders,
            devMode() && health?.dialog ? tr.dashboard.gatewayNote(health.dialog) : '',
        ].filter(Boolean),
        links: [...sheetLink('think', context), { label: tr.dashboard.openCharacter, open: () => openSettings('character') }, { label: tr.dashboard.providersAndModels, open: () => openSettings('brain') }],
    };
}

/** Глубина Claude по ступеням мышления: быстро, с умом, глубоко. */
const EFFORT_OF: Record<Thinking, LlmEffort> = { fast: 'low', smart: 'medium', deep: 'high' };
const effortSub = (): Record<Thinking, string> => ({
    fast: tr.dashboard.effortFast,
    smart: tr.dashboard.effortSmart,
    deep: tr.dashboard.effortDeep,
});

/** Мышление: у Claude глубина выбирается здесь; у других моделей — только показываем, что есть. */
function thinkSpec({ data, nodes, openSettings }: SpecContext): Spec {
    const thinks = nodes.brain.tone !== 'yandex' && nodes.brain.tone !== 'off' && data.brain && data.brain.provider !== 'echo';
    const level: Thinking | null = thinks && data.brain ? thinkingOf(data.brain) : null;
    const claude = !!thinks && data.brain?.provider === 'anthropic';
    const fromPanel = data.brain?.source === 'panel';
    const mayChoose = can('models.manage') && claude && fromPanel;
    return {
        key: 'think',
        tone: nodes.think.tone,
        title: NODE_LABELS.think,
        about:
            nodes.brain.tone === 'yandex'
                ? tr.dashboard.thinkAlice
                : level
                  ? tr.dashboard.thinkHowLong
                  : tr.dashboard.brainNotChosenDot,
        choices: (Object.keys(THINKING) as Thinking[]).map((id) => ({
            id,
            name: THINKING[id].name,
            sub: claude ? effortSub()[id] : THINKING[id].sub,
            pill: { text: THINKING[id].pill, tone: 'off' as Tone },
            on: claude ? data.brain?.effort === EFFORT_OF[id] || (id === 'fast' && !data.brain?.effort) : level === id,
            disabled: !mayChoose,
            apply: async () => {
                await api.chooseBrain({ effort: EFFORT_OF[id] });
                return tr.dashboard.appliedNext;
            },
        })),
        notes: [
            level ? tr.dashboard.thinkNow(nodes.think.detail) : '',
            claude && !fromPanel ? tr.dashboard.effortFromEnv : '',
            thinks && !claude ? tr.dashboard.effortClaudeOnly : '',
        ].filter(Boolean),
        links: [{ label: tr.dashboard.fastAndSmartModels, open: () => openSettings('brain') }],
    };
}

/** Характер: готовые карточки применяются сразу, со следующего вопроса; свой пишут в «Мозг» → «Персонажи». */
function personaSpec({ data, nodes, openSettings }: SpecContext): Spec {
    const info = data.persona;
    const mayEdit = can('persona.edit');
    const blocked = data.mode === 'relay' || nodes.brain.tone === 'yandex';
    const choices: Choice[] = (info?.presets ?? []).map((preset) => ({
        id: preset.id,
        name: preset.title,
        sub: preset.description,
        on: info?.activePreset === preset.id,
        disabled: blocked || !mayEdit,
        apply: async () => {
            await api.setPreset(preset.id);
            return tr.dashboard.fromNextQuestion;
        },
    }));
    if (info && info.activePreset === null)
        choices.unshift({
            id: 'custom',
            name: nodes.persona.tone === 'off' ? tr.dashboard.custom : nodes.persona.value,
            sub: info.source === 'file' ? tr.dashboard.personaFromFile : tr.dashboard.personaOwnText,
            on: true,
            disabled: true,
        });
    return {
        key: 'persona',
        tone: nodes.persona.tone,
        title: NODE_LABELS.persona,
        about:
            data.mode === 'relay'
                ? relayAbout()
                : nodes.brain.tone === 'yandex'
                  ? tr.dashboard.personaAliceOwn
                  : byMode(tr.dashboard.personaAboutStation, tr.dashboard.personaAboutDevice),
        choices,
        empty: info ? undefined : tr.dashboard.readingPersona,
        notes: [!mayEdit && !blocked ? adminOnly() : ''].filter(Boolean),
        links: mayEdit ? [{ label: tr.dashboard.charactersAndOwn, open: () => openSettings('characters') }] : [],
    };
}

/** Слух: что распознаёт сейчас, что ждёт перезапуска, и Яндекс — это уход всей колонки в режим «Яндекс». */
function earSpec({ data, nodes, health, openSettings }: SpecContext): Spec {
    const view = data.models;
    const relay = data.mode === 'relay';
    const mayModels = can('models.manage');
    const tone: Tone = nodes.ear.tone === 'cloud' ? 'cloud' : 'own';
    const choices: Choice[] = [];
    if (view) {
        const active = view.asr.find((card) => card.active);
        const waiting = view.asr.find((card) => card.pending && !card.active);
        if (active)
            choices.push({
                id: active.id,
                name: shortTitle(active.title),
                sub: active.description,
                pill: { text: WHERE[tone], tone },
                on: !relay,
                disabled: relay,
            });
        else if (view.asrLoaded)
            choices.push({
                id: 'loaded',
                name: view.asrLoaded,
                sub: tr.dashboard.modelNotInCatalog,
                pill: { text: WHERE[tone], tone },
                on: !relay,
                disabled: relay,
            });
        if (waiting && view.localAsr)
            // На этом компьютере новая модель скачивается и встаёт сама, без перезапуска.
            choices.push({
                id: waiting.id,
                name: shortTitle(waiting.title),
                sub: view.localAsr.phase === 'failed' ? (view.localAsr.error ?? tr.dashboard.didNotStart) : tr.dashboard.chosenLocal(localAsrLine(view.localAsr)),
                pill: { text: view.localAsr.phase === 'failed' ? tr.dashboard.failedPill : tr.dashboard.preparingPill, tone: 'off' },
                on: false,
                disabled: true,
            });
        else if (waiting)
            choices.push({
                id: waiting.id,
                name: shortTitle(waiting.title),
                sub: view.canRestart ? tr.dashboard.waitingRestartStation : tr.dashboard.waitingRestartServer,
                pill: { text: tr.dashboard.waitsPill, tone: 'off' },
                on: false,
                disabled: relay || !mayModels || !view.canRestart,
                confirm: tr.dashboard.restartRecognition(shortTitle(waiting.title)),
                apply: async () => {
                    await api.restartServer('asr');
                    return tr.dashboard.earRestarted;
                },
            });
        choices.push({
            id: 'yandex',
            name: tr.dashboard.yandexWord,
            sub: data.mode === 'own' ? tr.dashboard.notAllowedInOwn : tr.dashboard.stationGoesToYandex,
            pill: { text: WHERE.yandex, tone: 'yandex' },
            on: relay,
            disabled: relay || data.mode === 'own' || !can('station.mode'),
            confirm: tr.dashboard.becomesYandexEar(nodes.ear.value),
            apply: async () => {
                await api.setMode('relay');
                return tr.dashboard.stationReconnecting;
            },
        });
    }
    return {
        key: 'ear',
        tone: nodes.ear.tone,
        title: NODE_LABELS.ear,
        about: relay ? tr.dashboard.earAboutRelay : tr.dashboard.earAbout,
        choices,
        empty: view ? undefined : tr.dashboard.readingModels,
        instead: view ? undefined : data.modelsAbsent ? tr.dashboard.hearingOnComputer : data.modelsFailed ? tr.dashboard.hearingUnread : undefined,
        notes: [view && !mayModels && !relay ? adminOnly() : '', devMode() && health?.speech?.asr ? tr.dashboard.gatewayNote(health.speech.asr) : ''].filter(Boolean),
        links: [{ label: tr.dashboard.allRecognitionModels, open: () => openSettings('recognition') }],
    };
}

/** Голос: сверху голос Алисы, под ним свои голоса (отпечатки), дальше скачанные; скачать новые — на экране «Голос». */
/** Ответ «своих моделей здесь нет» (501): шлюз на колонке. */
const ABSENT = Symbol('absent');

/** Сколько окна ждут голоса и слух центра. */
const MODELS_WAIT_MS = 10_000;

/** Обещание с пределом ожидания: не дождались — отказ. */
function within<T>(promise: Promise<T>, ms: number): Promise<T> {
    return new Promise((resolve, reject) => {
        const timer = window.setTimeout(() => reject(new Error('timeout')), ms);
        promise.then(
            (value) => {
                window.clearTimeout(timer);
                resolve(value);
            },
            (error: unknown) => {
                window.clearTimeout(timer);
                reject(error);
            },
        );
    });
}

function voiceSpec(context: SpecContext): Spec {
    const { data, nodes, health, openSettings } = context;
    const view = data.models;
    const relay = data.mode === 'relay';
    const mayModels = can('models.manage');
    const tone: Tone = data.connectors?.find((entry) => entry.manifest.id === 'tts')?.manifest.cloud ? 'cloud' : 'own';
    const activate = (modelId: string, voiceId?: string) => async (anyway?: boolean): Promise<string> => {
        const { needsRestart } = await api.activateTts(modelId, voiceId, anyway);
        if (needsRestart && view?.canRestart) {
            await api.restartServer('tts');
            return tr.dashboard.voiceServerRestarted;
        }
        return modelId === 'yandex' ? tr.dashboard.aliceSpeaks : tr.dashboard.speaksNextAnswer;
    };
    const choices: Choice[] = [];
    if (view) {
        choices.push({
            id: 'yandex',
            name: tr.dashboard.alice,
            sub: data.mode === 'own' ? tr.dashboard.voiceAliceInOwn : tr.dashboard.yandexVoice,
            pill: { text: WHERE.yandex, tone: 'yandex' },
            on: view.voice === 'yandex',
            disabled: relay || !mayModels,
            apply: activate('yandex'),
        });
        const ready = view.tts.filter((card) => card.engine !== 'yandex' && card.installed && card.loaded);
        for (const card of [...ready.filter((entry) => entry.group === 'own'), ...ready.filter((entry) => entry.group !== 'own')]) {
            const voices = card.engine === 'silero' ? card.voices.map((voice) => voice.id) : [undefined];
            for (const voiceId of voices)
                choices.push({
                    id: voiceId ?? card.id,
                    name: voiceName(card, voiceId),
                    sub:
                        card.group === 'own'
                            ? tr.dashboard.ownEngine(card.own?.engineTitle ?? card.engine)
                            : card.engine === 'silero'
                              ? `${shortTitle(card.title)} · ${voiceId?.split('/').pop() ?? ''}`
                              : `Piper${card.quality ? ` · ${card.quality}` : ''}`,
                    pill: { text: WHERE[tone], tone },
                    on: view.voice === 'own' && card.active && (!voiceId || view.ttsActive.voiceId === voiceId),
                    disabled: relay || !mayModels,
                    apply: activate(card.id, voiceId),
                });
        }
    }
    const own = choices.filter((choice) => choice.id !== 'yandex').length;
    return {
        key: 'voice',
        tone: nodes.voice.tone,
        title: NODE_LABELS.voice,
        about: relay ? tr.dashboard.voiceAboutRelay : byMode(tr.dashboard.voiceAboutStation, tr.dashboard.voiceAboutDevice),
        choices,
        empty: view ? undefined : tr.dashboard.readingVoices,
        instead: view ? undefined : data.modelsAbsent ? tr.dashboard.voicesOnComputer : data.modelsFailed ? tr.dashboard.voicesUnread : undefined,
        notes: [
            view && !view.ownVoiceServer ? byMode(tr.dashboard.noOwnVoiceServerStation, tr.dashboard.noOwnVoiceServerDevice) : '',
            view?.ownVoiceServer && !own ? tr.dashboard.noOwnVoices : '',
            view && !mayModels && !relay ? adminOnly() : '',
            devMode() && health?.tts ? tr.dashboard.gatewayNote(health.tts) : '',
        ].filter(Boolean),
        links: [...sheetLink('persona', context), { label: tr.dashboard.openCharacter, open: () => openSettings('character') }, { label: tr.dashboard.downloadMoreVoices, open: () => openSettings('voice') }],
    };
}

function SheetIcon({ icon: SheetGlyph, tone }: { icon: Icon; tone: Tone }) {
    return (
        <span class={`dash-circle dash-sheet-icon ${toneClass(tone)}`} aria-hidden="true">
            <SheetGlyph size={20} />
        </span>
    );
}

function ChoiceSheet({ spec, onClose, onApplied }: { spec: Spec; onClose: () => void; onApplied: () => void }) {
    const [pending, setPending] = useState<Choice | null>(null);
    /** Шлюз не включил: памяти впритык. Не ошибка, а вопрос — включить всё равно? */
    const [tight, setTight] = useState<{ choice: Choice; text: string } | null>(null);
    const [busy, setBusy] = useState<string | null>(null);
    const [error, setError] = useState<string | null>(null);
    const [notice, setNotice] = useState<string | null>(null);

    const run = async (choice: Choice, anyway = false): Promise<void> => {
        if (!choice.apply) return;
        setPending(null);
        setTight(null);
        setBusy(choice.id);
        setError(null);
        setNotice(null);
        try {
            setNotice(await choice.apply(anyway));
            onApplied();
        } catch (e) {
            if (isTight(e)) setTight({ choice, text: problemOf(e) });
            else setError(e instanceof ApiError || !(e instanceof Error) ? problemOf(e) : e.message);
        } finally {
            setBusy(null);
        }
    };

    const pick = (choice: Choice): void => {
        if (choice.on || choice.disabled || busy) return;
        setTight(null);
        if (choice.confirm) return setPending(choice);
        void run(choice);
    };

    // «Станет … вместо …» and «включить всё равно?» wait below, as the window's one action: «Отменить · Применить».
    const question = pending?.confirm
        ? { text: pending.confirm, ok: tr.dashboard.apply, yes: () => void run(pending), no: () => setPending(null) }
        : tight
          ? { text: tr.dashboard.voiceNotEnabled(tight.text), ok: tr.dashboard.enableAnyway, yes: () => void run(tight.choice, true), no: () => setTight(null) }
          : null;

    return (
        <Sheet
            title={spec.title}
            icon={<SheetIcon icon={NODE_ICONS[spec.key]} tone={spec.tone} />}
            onClose={onClose}
            footer={
                question ? (
                    <>
                        <span class="win-dirty" role="status">
                            {question.text}
                        </span>
                        <span class="win-grow" />
                        <button class="btn btn-plain" onClick={question.no}>
                            {tr.dashboard.cancel}
                        </button>
                        <button class="btn btn-main" onClick={question.yes}>
                            {question.ok}
                        </button>
                    </>
                ) : undefined
            }
        >
            <p class="win-lead">{spec.about}</p>
            {spec.instead ? (
                <p class="win-text m-0" role="status">
                    {spec.instead}
                </p>
            ) : spec.empty ? (
                <SkeletonRows rows={3} label={spec.empty} />
            ) : (
                <div role="radiogroup" aria-label={spec.title} class="flex flex-col">
                    {spec.choices.map((choice) => (
                        <button
                            key={choice.id}
                            role="radio"
                            aria-checked={choice.on}
                            disabled={(choice.disabled && !choice.on) || (!!busy && busy !== choice.id)}
                            class={`dash-option${pending?.id === choice.id || tight?.choice.id === choice.id ? ' pending' : ''}${choice.disabled && !choice.on ? ' opacity-45' : ''}`}
                            onClick={() => pick(choice)}
                        >
                            {choice.brand && choice.brand in BRANDS && (
                                <span class="route-icon w-32 h-32 flex-none" aria-hidden="true">
                                    <BrandMark id={choice.brand as BrandId} size={16} />
                                </span>
                            )}
                            <span class="dash-option-text">
                                <b>{choice.name}</b>
                                <span class="t-caption">{choice.sub}</span>
                            </span>
                            {choice.pill && <span class={pillClass(choice.pill.tone)}>{choice.pill.text}</span>}
                            <span class={`dash-radio${choice.on ? ' on' : ''}`} aria-hidden="true">
                                {busy === choice.id ? <span class="spinner" /> : choice.on ? <IconCheck /> : null}
                            </span>
                        </button>
                    ))}
                </div>
            )}
            {error && (
                <p role="alert" class="win-callout m-0 wrap-anywhere">
                    {error}
                </p>
            )}
            {notice && (
                <p role="status" class="win-text m-0 c-text">
                    {notice}
                </p>
            )}
            {spec.notes.map((note) => (
                <p key={note} class="win-note wrap-anywhere">
                    {note}
                </p>
            ))}
            <GoRows links={spec.links} onGo={onClose} />
        </Sheet>
    );
}

/** Where else this is set: rows that lead there («Провайдеры и модели», «Все подключения»). */
function GoRows({ links, onGo }: { links: Spec['links']; onGo: () => void }) {
    if (links.length === 0) return null;
    return (
        <div class="win-group">
            {links.map((link) => (
                // Переход из окна закрывает окно: страница открывалась позади него (проверка начисто 18.11, п. 27).
                <button
                    key={link.label}
                    class="win-row win-choice text-left"
                    onClick={() => {
                        onGo();
                        link.open();
                    }}
                >
                    <span class="win-row-main">
                        <span class="win-row-t">{link.label}</span>
                    </span>
                    <IconChevronRight size={16} />
                </button>
            ))}
        </div>
    );
}

/** Подключения: что шлюз умеет — с настоящим состоянием; чего ещё не умеет — заглушкой «скоро». */
function LinksSheet({
    sats,
    sleeping,
    loaded,
    allowed,
    onClose,
    onOpenHouse,
}: {
    sats: SatView[];
    sleeping: boolean;
    loaded: boolean;
    allowed: boolean;
    onClose: () => void;
    onOpenHouse: (connector?: string) => void;
}) {
    const rows: Array<{ id: string; name: string; sub: string; pill: string; cls: string; setup?: boolean }> = [];
    for (const sat of sats) {
        for (const entry of sat.all)
            rows.push({
                id: entry.manifest.id,
                name: entry.manifest.title,
                sub: entry.status.detail ?? entry.manifest.description,
                // Настраивается в «Коннекторах»: строка ведёт в его лист.
                setup: entry.manifest.setup === 'panel' && can('connectors.manage'),
                ...connectorPill(entry),
            });
        const stub = sat.stub ? STUBS[sat.key] : undefined;
        if (stub) rows.push({ id: sat.key, name: sat.title, sub: stub.sub, pill: stub.pill, cls: 'pill pill-off' });
    }
    return (
        <Sheet
            title={tr.dashboard.linksTitle}
            about={sleeping ? tr.dashboard.linksAboutRelay : byMode(tr.dashboard.linksAboutStation, tr.dashboard.linksAboutDevice)}
            icon={<SheetIcon icon={IconLinks} tone={sleeping ? 'off' : 'own'} />}
            onClose={onClose}
        >
            {!allowed ? (
                <p class="win-callout is-calm m-0">{tr.dashboard.linksAdminOnly}</p>
            ) : !loaded ? (
                <SkeletonRows rows={4} label={tr.dashboard.linksLoading} />
            ) : (
                rows.length > 0 && (
                    <div class="win-group">
                        {rows.map((row) => {
                            const body = (
                                <>
                                    <span class="win-row-main">
                                        <span class="win-row-t">{row.name}</span>
                                        <span class="win-row-s">{row.sub}</span>
                                    </span>
                                    <span class={row.cls}>{row.pill}</span>
                                </>
                            );
                            return row.setup ? (
                                <button key={row.id} class="win-row win-choice text-left" onClick={() => onOpenHouse(row.id)}>
                                    {body}
                                </button>
                            ) : (
                                <div key={row.id} class="win-row">
                                    {body}
                                </div>
                            );
                        })}
                    </div>
                )
            )}
            {allowed && <GoRows links={[{ label: tr.dashboard.allLinks, open: () => onOpenHouse() }]} onGo={onClose} />}
        </Sheet>
    );
}

/** Плашка подключения: не настроено и выключено — сланец, нет связи — коралл, скоро — нет в этой сборке, иначе где оно: дома, облако, помощник. */
function connectorPill(entry: ConnectorView): { pill: string; cls: string } {
    if (entry.status.state === 'soon') return { pill: tr.dashboard.pillSoon, cls: 'pill pill-off' };
    if (entry.status.state === 'unset') return { pill: tr.dashboard.pillUnset, cls: 'pill pill-off' };
    if (entry.status.state === 'off') return { pill: tr.dashboard.pillOff, cls: 'pill pill-off' };
    if (entry.status.state === 'error') return { pill: tr.dashboard.pillNoLink, cls: 'pill pill-error' };
    if (entry.manifest.kind === 'agent') return { pill: WHERE.agent, cls: 'pill pill-agent' };
    return entry.manifest.cloud ? { pill: WHERE.cloud, cls: 'pill pill-yandex' } : { pill: WHERE.own, cls: 'pill pill-own' };
}

/**
 * Над орбитой: кто на деле слышит, думает и говорит сейчас (решение 4). Три плашки по умолчанию скрыты — места
 * больше; раскрывает и закрывает подчёркнутое слово в конце предложения о статусе. Выбранный мозг не отвечает —
 * янтарная подсказка почему.
 */
function TruthStrip({ nodes, truth, mode, open, onToggle }: { nodes: Record<NodeKey, NodeView>; truth: Solved; mode: PanelMode; open: boolean; onToggle: () => void }) {
    const who = (node: NodeView): Who => ({ host: null, value: node.value, sub: node.detail, tone: node.tone === 'agent' ? 'own' : node.tone });
    const changed = truth.mode !== mode ? tr.dashboard.nowInstead(MODE_TITLES[truth.mode], MODE_TITLES[mode]) : '';
    const note = (truth.notes.join(' ') + changed).trim();
    const panel = usePanel();
    // Скрыта и сказать нечего — строки нет совсем; открыть можно словом в подписи режима над дашбордом.
    if (!open && !note) return null;
    return (
        <section class="truth-strip" aria-label={tr.dashboard.whoAnswersNow}>
            {open && <WhoThree ear={who(nodes.ear)} brain={who(nodes.brain)} voice={who(nodes.voice)} />}
            {note && (
                <p class="truth-note">
                    {note}{' '}
                    {/* Режим «Микрофон»: чего не хватает — сразу туда, где это ставят. */}
                    {truth.fixes?.map((fix) => (
                        <span key={fix.label}>
                            <button class="truth-more" onClick={() => panel.go(...fix.place)}>
                                {fix.label}
                            </button>{' '}
                        </span>
                    ))}
                    <button class="truth-more" aria-expanded={open} onClick={onToggle}>
                        {open ? tr.dashboard.collapse : tr.dashboard.more}
                    </button>
                </p>
            )}
        </section>
    );
}
