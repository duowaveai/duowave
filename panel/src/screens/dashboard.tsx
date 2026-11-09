import { FunctionComponent } from 'preact';
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
    IconCheck,
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
import { shade, useReducedMotion } from '../stations/kit';
import { useTheme } from '../shell/theme';
import { safeColor } from '../ui/media';
import { useAvailability } from '../availability';
import { BrandMark } from '../ui/brand';
import { BRANDS, type BrandId } from '../brand-icons';
import { Sheet } from '../ui/sheet';
import {
    DashData,
    LINE,
    LIT,
    NODE_LABELS,
    NodeKey,
    NodeView,
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
import type { SettingsTab } from './mind';
import { DONE_MS, liveTripOf } from './trip';
import { MODE_TITLES, Solved, Who } from '../truth';
import { WhoThree } from './placement-map';

/** How far the stage's glow reaches past its core («Минимал»): what the blur(60px) spread was. */
const GLOW_REACH = 110;

// Дашборд колонки: колонка в центре своей орбиты, вокруг — мозг, мышление, характер, слух и голос, снизу —
// капсула подключений. Образ — docs/design/glass/source/GlassDash.dc.html; сцена считается под экран
// (dash-scene.ts): орбита и колонка — по высоте, что остаётся до панели вкладок, подписи — всегда одного размера.
// Под колонкой во всех темах — карточка ответа: кто работает, что делает, волна голоса, пока колонка говорит.
// В «Минимале» (look: 'minimal') за колонкой ещё свечение цветом того, кто отвечает, персонаж влетает в колонку,
// а при смене колонки новая модель выезжает слева (DESIGN.md, «Минимал» → Motion; ui/minimal.css).

/** Ширина узла: мозг — посередине сверху, остальные — по бокам. */
const NODE_W: Record<NodeKey, number> = { brain: 120, persona: 98, think: 98, ear: 98, voice: 98 };
/** У слуха и голоса круг сверху, подпись под ним; у остальных — подпись над кругом. */
const BELOW: ReadonlySet<NodeKey> = new Set(['ear', 'voice']);
const ORDER: NodeKey[] = ['brain', 'persona', 'think', 'ear', 'voice'];

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

/** Сцена под экран: ширина — от коробки, высота — сколько остаётся до панели вкладок, пока страница вверху. */
function useScene(): [preact.RefObject<HTMLDivElement>, Scene] {
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
        const room = Math.round(bottom - top);
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

export function Dashboard({ health, session, drawing, online, trip, skew = 0, version = 0, onOpenSettings, onOpenHouse, whoOpen, onToggleWho }: Props) {
    const mode: PanelMode = health?.panelMode ?? (health?.mode === 'relay' ? 'relay' : 'gateway');
    const [models, setModels] = useState<ModelsView | null>(null);
    // На колонке слух и голос — по списку шлюза: у компьютера, в облаке, у Яндекса.
    const availability = useAvailability();
    const [persona, setPersona] = useState<PersonaInfo | null>(null);
    const [connectors, setConnectors] = useState<ConnectorView[] | null>(null);
    const [brainView, setBrainView] = useState<BrainView | null>(null);
    const [characters, setCharacters] = useState<CharactersView | null>(null);
    const [stage, setStage] = useState<TripStage>('idle');
    const [sheet, setSheet] = useState<NodeKey | 'links' | null>(null);
    const [, setTick] = useState(0);
    const minimal = useTheme().theme.look === 'minimal';
    const reduced = useReducedMotion();
    const [box, scene] = useScene();
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
                can('station.view') ? api.models().catch(() => null) : null,
                can('station.view') ? api.persona().catch(() => null) : null,
                can('connectors.view')
                    ? api
                          .connectors()
                          .then((list) => list.connectors)
                          .catch(() => null)
                    : null,
                can('connectors.view') ? api.brain().catch(() => null) : null,
                wantCharacters.current && can('station.view') ? api.characters().catch(() => null) : null,
            ]);
            if (!alive.current) return;
            if (freshModels) setModels(freshModels);
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
        persona,
        connectors,
        availability,
    };
    const truth = truthOf(data);
    const nodes = nodesOf(data, truth);
    const sats = satellitesOf(data);
    const live = liveTripOf(trip, Date.now() + skew, nodes);
    const shown = live?.stage ?? stage;
    const lit = live?.lit ?? LIT[stage];
    const [caption, captionSub] = live?.caption ?? captionOf(stage, nodes, mode);
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
        ? 'Подключения спят'
        : !mayLinks
          ? 'Подключения'
          : connectors
            ? `Подключено · ${linked.length}`
            : 'Подключения…';

    /** Один вопрос по орбите: слух слышит, мозг думает, голос отвечает — и в конце видно, что ушло из дома. */
    const play = (): void => {
        timers.current.forEach((timer) => window.clearTimeout(timer));
        setStage('idle');
        timers.current = TRIP.map(([ms, next]) => window.setTimeout(() => setStage(next), ms));
    };

    const context: SpecContext = { data, nodes, health, openSettings: onOpenSettings };
    const { art } = scene;
    /** Рисунок колонки — 240×200 точек; нажимается её корпус. */
    const k = art.size / 240;
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

    return (
        <>
            <TruthStrip nodes={nodes} truth={truth} mode={mode} open={whoOpen} onToggle={onToggleWho} />
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
                            left: `${art.left + 17 * k}px`,
                            top: `${art.top + 17 * k}px`,
                            width: `${206 * k}px`,
                            height: `${176 * k}px`,
                            borderRadius: `${48 * k}px`,
                        }}
                        aria-label={live ? `${live.label}. Показать путь вопроса на примере` : 'Показать путь вопроса'}
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

                    <button
                        class="dash-links"
                        style={{ left: `${scene.width / 2}px`, top: `${scene.linksTop}px` }}
                        aria-label={`${
                            sleeping
                                ? 'Подключения спят в режиме «Яндекс»'
                                : connectors
                                  ? `Подключено ${linked.length}${linked.length ? `: ${linked.map((entry) => entry.manifest.title).join(', ')}` : ''}`
                                  : 'Подключения'
                        }. Открыть`}
                        onClick={() => setSheet('links')}
                    >
                        <span class="dash-sats" aria-hidden="true">
                            {sats.map((sat) => (
                                <Satellite key={sat.key} sat={sat} sleeping={sleeping} />
                            ))}
                        </span>
                        <span class={sleeping ? 'c-sub' : 'c-text'}>{linksCaption}</span>
                    </button>

                    {orb && !reduced && (
                        // Шар персонажа: в покое лежит под кругом характера, на ответе летит в кольцо колонки и гаснет.
                        <span
                            class={`dash-fly${inside ? ' inside' : ''}`}
                            aria-hidden="true"
                            style={{
                                left: `${scene.nodes.persona.x}px`,
                                top: `${scene.nodes.persona.y}px`,
                                '--fly-x': `${scene.ring.x - scene.nodes.persona.x}px`,
                                '--fly-y': `${scene.ring.y - scene.nodes.persona.y}px`,
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
                            node={nodes[key]}
                            at={scene.nodes[key]}
                            lit={lit.includes(key) && !(key === 'persona' && inside)}
                            orb={key === 'persona' ? orb : undefined}
                            emptied={key === 'persona' && inside}
                            onOpen={() => setSheet(key)}
                        />
                    ))}
                </div>
            </div>

            {sheet && sheet !== 'links' && (
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

function OrbitNode({
    node,
    at,
    lit,
    orb,
    emptied = false,
    onOpen,
}: {
    node: NodeView;
    at: Point;
    lit: boolean;
    /** «Минимал»: шар персонажа в круге характера (CSS-градиент). */
    orb?: string;
    /** Шар сейчас в колонке — круг пустеет. */
    emptied?: boolean;
    onOpen: () => void;
}) {
    const width = NODE_W[node.key];
    const below = BELOW.has(node.key);
    const NodeIcon = NODE_ICONS[node.key];
    const where = node.tone !== 'off' ? `, ${WHERE[node.tone]}` : '';
    const label = (
        <span class="dash-label">
            <small>{NODE_LABELS[node.key]}</small>
            <b>{node.value}</b>
        </span>
    );
    const circle = (
        <span class={`dash-circle ${toneClass(node.tone)}${lit ? ' on' : ''}${orb ? ' has-orb' : ''}${emptied ? ' emptied' : ''}`}>
            {orb && (
                <span class="dash-orb" style={{ '--g': orb }}>
                    <i />
                </span>
            )}
            <NodeIcon size={22} />
            {lit && <span class="dash-pulse dash-anim" />}
        </span>
    );
    return (
        <button
            class="dash-node"
            // Круг узла — ровно на орбите: у подписи сверху узел отсчитывается от низа круга.
            style={{
                left: `${at.x - width / 2}px`,
                top: `${below ? at.y - CIRCLE / 2 : at.y + CIRCLE / 2}px`,
                width: `${width}px`,
                transform: below ? undefined : 'translateY(-100%)',
            }}
            aria-label={`${NODE_LABELS[node.key]}: ${node.value}${where}. ${node.key === 'think' ? 'Подробнее' : 'Изменить'}`}
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
    notes: string[];
    links: Array<{ label: string; open: () => void }>;
}

interface SpecContext {
    data: DashData;
    nodes: Record<NodeKey, NodeView>;
    health: Health | null;
    openSettings: (tab: SettingsTab) => void;
}

const pillClass = (tone: Tone): string => `pill pill-${tone === 'cloud' ? 'yandex' : tone}`;
const RELAY_ABOUT = 'В режиме «Яндекс» всё решает Яндекс.';
const ADMIN_ONLY = 'Менять может администратор.';

/** Что показать в шите узла: варианты с тем, что можно здесь и сейчас, и честное «где это меняется» для остального. */
function specFor(key: NodeKey, context: SpecContext): Spec {
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
function brainSpec({ data, nodes, health, openSettings }: SpecContext): Spec {
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
    const alice: Choice = {
        id: 'yandex',
        name: 'Алиса',
        sub: data.mode === 'own' ? 'нельзя в режиме «Свой»' : 'отвечает Яндекс',
        pill: { text: WHERE.yandex, tone: 'yandex' },
        on: executor === 'yandex',
        disabled: relay || data.mode === 'own' || !mayRoute,
        confirm: `Станет «Алиса» вместо «${nodes.brain.value}»: разговор уйдёт Яндексу`,
        apply: async () => {
            await setFirst('yandex');
            return 'Применено — со следующего вопроса.';
        },
    };
    if (!view) {
        // Без права смотреть подключения: одна строка «наша модель», как раньше.
        return {
            key: 'brain',
            tone: nodes.brain.tone,
            title: NODE_LABELS.brain,
            about: relay ? RELAY_ABOUT : 'Кто думает над ответом — первый шаг маршрута «Разговор».',
            choices: [
                {
                    id: 'own',
                    name: ours.name,
                    sub: ours.detail,
                    pill: ours.usable ? { text: WHERE[ours.tone], tone: ours.tone } : undefined,
                    on: executor === 'own',
                    disabled: relay || !mayRoute || !ours.usable,
                    confirm: `Станет «${ours.name}» вместо «${nodes.brain.value}»`,
                    apply: async () => {
                        await setFirst('own');
                        return 'Применено — со следующего вопроса.';
                    },
                },
                alice,
            ],
            notes: [!mayRoute && !relay ? ADMIN_ONLY : '', health?.dialog ? `Шлюз: ${health.dialog}` : ''].filter(Boolean),
            links: [],
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
                sub: `${provider.title}${provider.check && !provider.check.ok ? ' · не прошёл проверку' : ''}`,
                brand: provider.brand,
                pill: { text: WHERE[tone], tone },
                on,
                disabled: relay || !mayChoose || (executor !== 'own' && !mayRoute),
                confirm: `Станет «${name}» вместо «${nodes.brain.value}»`,
                apply: async () => {
                    await setFirst('own');
                    await api.chooseBrain({ fast: { provider: provider.id, model } });
                    return 'Применено — со следующего вопроса.';
                },
            };
        });
    });
    const envChoice: Choice[] = view.env.usable
        ? [
              {
                  id: 'env',
                  name: `${view.env.provider === 'anthropic' ? claudeName(view.env.models.fast) : view.env.models.fast}`,
                  sub: `${view.env.title} · из .env`,
                  pill: { text: WHERE[view.env.cloud ? 'cloud' : 'own'], tone: view.env.cloud ? 'cloud' : 'own' },
                  on: executor === 'own' && !fast,
                  disabled: relay || !mayChoose || (executor !== 'own' && !mayRoute),
                  confirm: `Станет модель из .env вместо «${nodes.brain.value}»`,
                  apply: async () => {
                      await setFirst('own');
                      await api.chooseBrain({ fast: null });
                      return 'Применено — со следующего вопроса.';
                  },
              },
          ]
        : [];
    return {
        key: 'brain',
        tone: nodes.brain.tone,
        title: NODE_LABELS.brain,
        about: relay ? RELAY_ABOUT : 'Кто думает над ответом — первый шаг маршрута «Разговор».',
        choices: [...models, ...envChoice, alice],
        notes: [
            !mayChoose && !relay ? ADMIN_ONLY : '',
            models.length ? '' : 'Провайдеров пока нет: добавьте свой ключ в «Мозг» → «Модели».',
            health?.dialog ? `Шлюз: ${health.dialog}` : '',
        ].filter(Boolean),
        links: [{ label: 'Провайдеры и все модели', open: () => openSettings('brain') }],
    };
}

/** Глубина Claude по ступеням мышления: быстро, с умом, глубоко. */
const EFFORT_OF: Record<Thinking, LlmEffort> = { fast: 'low', smart: 'medium', deep: 'high' };
const EFFORT_SUB: Record<Thinking, string> = {
    fast: 'думает меньше всех, первое слово за секунду',
    smart: 'думает чуть дольше, точнее',
    deep: 'думает дольше всех над сложным',
};

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
                ? 'Думает Алиса — её глубину не выбрать.'
                : level
                  ? 'Сколько думать перед ответом.'
                  : 'Мозг не выбран.',
        choices: (Object.keys(THINKING) as Thinking[]).map((id) => ({
            id,
            name: THINKING[id].name,
            sub: claude ? EFFORT_SUB[id] : THINKING[id].sub,
            pill: { text: THINKING[id].pill, tone: 'off' as Tone },
            on: claude ? data.brain?.effort === EFFORT_OF[id] || (id === 'fast' && !data.brain?.effort) : level === id,
            disabled: !mayChoose,
            apply: async () => {
                await api.chooseBrain({ effort: EFFORT_OF[id] });
                return 'Применено — со следующего вопроса.';
            },
        })),
        notes: [
            level ? `Сейчас: ${nodes.think.detail}.` : '',
            claude && !fromPanel ? 'Глубину модели из .env задаёт LLM_EFFORT; выберите Claude в «Мозг» → «Модели», и она станет меняться здесь.' : '',
            thinks && !claude ? 'Глубина выбирается только у Claude. Умную модель для сложного выбирают в «Мозг» → «Модели».' : '',
        ].filter(Boolean),
        links: [{ label: 'Быстрая и умная модели', open: () => openSettings('brain') }],
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
            return 'Со следующего вопроса.';
        },
    }));
    if (info && info.activePreset === null)
        choices.unshift({
            id: 'custom',
            name: nodes.persona.value === 'нет' ? 'Свой' : nodes.persona.value,
            sub: info.source === 'file' ? 'из файла DIALOG_SYSTEM_PROMPT_FILE' : 'свой текст — правится в «Мозг» → «Персонажи»',
            on: true,
            disabled: true,
        });
    return {
        key: 'persona',
        tone: nodes.persona.tone,
        title: NODE_LABELS.persona,
        about:
            data.mode === 'relay'
                ? RELAY_ABOUT
                : nodes.brain.tone === 'yandex'
                  ? 'Отвечает Алиса — со своим характером.'
                  : 'Кем колонка отвечает, со следующего вопроса.',
        choices,
        empty: info ? undefined : 'Читаем характер…',
        notes: [!mayEdit && !blocked ? ADMIN_ONLY : ''].filter(Boolean),
        links: mayEdit ? [{ label: 'Персонажи и свой характер', open: () => openSettings('characters') }] : [],
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
                sub: 'модель не из каталога',
                pill: { text: WHERE[tone], tone },
                on: !relay,
                disabled: relay,
            });
        if (waiting)
            choices.push({
                id: waiting.id,
                name: shortTitle(waiting.title),
                sub: view.canRestart ? 'выбрана — заработает после перезапуска' : 'выбрана — заработает, когда сервер перезапустят',
                pill: { text: 'ждёт', tone: 'off' },
                on: false,
                disabled: relay || !mayModels || !view.canRestart,
                confirm: `Перезапустить распознавание с «${shortTitle(waiting.title)}»? Новая модель может скачиваться минуты.`,
                apply: async () => {
                    await api.restartServer('asr');
                    return 'Распознавание перезапущено.';
                },
            });
        choices.push({
            id: 'yandex',
            name: 'Яндекс',
            sub: data.mode === 'own' ? 'нельзя в режиме «Свой»' : 'колонка уходит к Яндексу целиком',
            pill: { text: WHERE.yandex, tone: 'yandex' },
            on: relay,
            disabled: relay || data.mode === 'own' || !can('station.mode'),
            confirm: `Станет «Яндекс» вместо «${nodes.ear.value}»: колонка уйдёт к Яндексу целиком — режим «Яндекс»`,
            apply: async () => {
                await api.setMode('relay');
                return 'Колонка переподключается к Яндексу…';
            },
        });
    }
    return {
        key: 'ear',
        tone: nodes.ear.tone,
        title: NODE_LABELS.ear,
        about: relay ? 'Сейчас слушает Яндекс; свою модель вернут «Шлюз» и «Свой».' : 'Кто превращает речь в текст.',
        choices,
        empty: view ? undefined : 'Читаем модели…',
        notes: [view && !mayModels && !relay ? ADMIN_ONLY : '', health?.speech?.asr ? `Шлюз: ${health.speech.asr}` : ''].filter(Boolean),
        links: [{ label: 'Все модели распознавания', open: () => openSettings('recognition') }],
    };
}

/** Голос: сверху голос Алисы, под ним свои голоса (отпечатки), дальше скачанные; скачать новые — на экране «Голос». */
function voiceSpec({ data, nodes, health, openSettings }: SpecContext): Spec {
    const view = data.models;
    const relay = data.mode === 'relay';
    const mayModels = can('models.manage');
    const tone: Tone = data.connectors?.find((entry) => entry.manifest.id === 'tts')?.manifest.cloud ? 'cloud' : 'own';
    const activate = (modelId: string, voiceId?: string) => async (anyway?: boolean): Promise<string> => {
        const { needsRestart } = await api.activateTts(modelId, voiceId, anyway);
        if (needsRestart && view?.canRestart) {
            await api.restartServer('tts');
            return 'Голосовой сервер перезапущен — голос звучит.';
        }
        return modelId === 'yandex' ? 'Говорит Алиса.' : 'Говорит со следующего ответа.';
    };
    const choices: Choice[] = [];
    if (view) {
        choices.push({
            id: 'yandex',
            name: 'Алиса',
            sub: data.mode === 'own' ? 'в «Своём» вместо неё говорит наш' : 'голос Яндекса',
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
                            ? `свой · ${card.own?.engineTitle ?? card.engine}`
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
        about: relay ? 'В режиме «Яндекс» говорит Алиса.' : 'Чем колонка произносит ответ.',
        choices,
        empty: view ? undefined : 'Читаем голоса…',
        notes: [
            view && !view.ownVoiceServer ? 'Свой голосовой сервер не задан (TTS_OWN_URL): можно только голос Алисы.' : '',
            view?.ownVoiceServer && !own ? 'Своих голосов на сервере пока нет — скачайте на экране «Голос».' : '',
            view && !mayModels && !relay ? ADMIN_ONLY : '',
            health?.tts ? `Шлюз: ${health.tts}` : '',
        ].filter(Boolean),
        links: [{ label: 'Скачать другие голоса', open: () => openSettings('voice') }],
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

    return (
        <Sheet title={spec.title} about={spec.about} icon={<SheetIcon icon={NODE_ICONS[spec.key]} tone={spec.tone} />} onClose={onClose}>
            {spec.empty ? (
                <p class="t-meta m-0">
                    {spec.empty}
                </p>
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
            {pending?.confirm && (
                <div role="status" class="dash-confirm">
                    <p>{pending.confirm}</p>
                    <div class="flex gap-8">
                        <button class="pill pill-own dash-confirm-btn" onClick={() => void run(pending)}>
                            Применить
                        </button>
                        <button class="pill pill-off dash-confirm-btn" onClick={() => setPending(null)}>
                            Отменить
                        </button>
                    </div>
                </div>
            )}
            {tight && (
                <div role="status" class="dash-confirm">
                    <p>Голос не включён. {tight.text}</p>
                    <div class="flex gap-8">
                        <button class="pill pill-own dash-confirm-btn" onClick={() => void run(tight.choice, true)}>
                            Включить всё равно
                        </button>
                        <button class="pill pill-off dash-confirm-btn" onClick={() => setTight(null)}>
                            Отменить
                        </button>
                    </div>
                </div>
            )}
            {error && (
                <p role="alert" class="t-meta m-0 c-dot-offline">
                    {error}
                </p>
            )}
            {notice && (
                <p role="status" class="t-meta m-0 c-text">
                    {notice}
                </p>
            )}
            {spec.notes.map((note) => (
                <p key={note} class="t-caption m-0 wrap-anywhere">
                    {note}
                </p>
            ))}
            {spec.links.map((link) => (
                <button key={link.label} class="dash-sheet-link" onClick={link.open}>
                    {link.label}
                </button>
            ))}
        </Sheet>
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
            title="Подключено"
            about={sleeping ? 'В режиме «Яндекс» подключения спят.' : 'Кого колонка может позвать на помощь.'}
            icon={<SheetIcon icon={IconLinks} tone={sleeping ? 'off' : 'own'} />}
            onClose={onClose}
        >
            {!allowed ? (
                <p class="t-meta m-0">
                    Подключения видят администратор и домашние.
                </p>
            ) : !loaded ? (
                <p class="t-meta m-0">
                    Читаем подключения…
                </p>
            ) : (
                <div class="flex flex-col">
                    {rows.map((row) => {
                        const body = (
                            <>
                                <span class="dash-option-text">
                                    <b>{row.name}</b>
                                    <span class="t-caption">{row.sub}</span>
                                </span>
                                <span class={row.cls}>{row.pill}</span>
                            </>
                        );
                        return row.setup ? (
                            <button key={row.id} class="dash-option cursor-pointer min-h-54 w-full" onClick={() => onOpenHouse(row.id)}>
                                {body}
                            </button>
                        ) : (
                            <div key={row.id} class="dash-option cursor-default min-h-54">
                                {body}
                            </div>
                        );
                    })}
                </div>
            )}
            {allowed && (
                <button class="dash-sheet-link" onClick={() => onOpenHouse()}>
                    Все подключения
                </button>
            )}
        </Sheet>
    );
}

/** Плашка подключения: не настроено и выключено — сланец, нет связи — коралл, скоро — нет в этой сборке, иначе где оно: дома, облако, помощник. */
function connectorPill(entry: ConnectorView): { pill: string; cls: string } {
    if (entry.status.state === 'soon') return { pill: 'скоро', cls: 'pill pill-off' };
    if (entry.status.state === 'unset') return { pill: 'не настроено', cls: 'pill pill-off' };
    if (entry.status.state === 'off') return { pill: 'выкл', cls: 'pill pill-off' };
    if (entry.status.state === 'error') return { pill: 'нет связи', cls: 'pill pill-error' };
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
    const changed = truth.mode !== mode ? ` · сейчас «${MODE_TITLES[truth.mode]}» вместо «${MODE_TITLES[mode]}»` : '';
    const note = (truth.notes.join(' ') + changed).trim();
    // Скрыта и сказать нечего — строки нет совсем; открыть можно словом в подписи режима над дашбордом.
    if (!open && !note) return null;
    return (
        <section class="truth-strip" aria-label="Кто отвечает сейчас">
            {open && <WhoThree ear={who(nodes.ear)} brain={who(nodes.brain)} voice={who(nodes.voice)} />}
            {note && (
                <p class="truth-note">
                    {note}{' '}
                    <button class="truth-more" aria-expanded={open} onClick={onToggle}>
                        {open ? 'свернуть' : 'подробнее'}
                    </button>
                </p>
            )}
        </section>
    );
}
