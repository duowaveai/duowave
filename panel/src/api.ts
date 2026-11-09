/**
 * Типизированный клиент шлюза: его API — /api/v1 (вход, люди, колонка, модели, подключения) и /health.
 * Ответы шлюза обёрнуты в { message, code } — снимаем обёртку. Что можно этому устройству, говорит /auth/me:
 * пульт прячет то, на что нет права, а шлюз всё равно проверяет каждое действие сам.
 */

import type { AvailabilityView, Placement } from './availability';

export class ApiError extends Error {
    constructor(
        readonly status: number,
        readonly body: string,
    ) {
        super(`API ${status}`);
    }
}

/** Сессия пульта: токен на месяц и кто вошёл — с правами, как их назвал шлюз. */
export interface PanelSession {
    token: string;
    name: string;
    role?: Role;
    permissions?: string[];
}

export type Role = 'admin' | 'member' | 'guest';

export interface PanelUser {
    id?: string;
    name: string;
    role: Role;
}

const SESSION_KEY = 'glass-session';

export const sessionStore = {
    get: (): PanelSession | null => {
        try {
            const raw = localStorage.getItem(SESSION_KEY);
            return raw ? (JSON.parse(raw) as PanelSession) : null;
        } catch {
            return null;
        }
    },
    set: (session: PanelSession): void => {
        try {
            localStorage.setItem(SESSION_KEY, JSON.stringify(session));
        } catch {
            // приватное окно: сессия живёт до перезагрузки страницы
        }
    },
    clear: (): void => {
        try {
            localStorage.removeItem(SESSION_KEY);
        } catch {
            // нечего чистить
        }
    },
};

/** Есть ли у вошедшего право (по ответу /auth/me; `*` у администратора разворачивается шлюзом). */
export function can(permission: string): boolean {
    return sessionStore.get()?.permissions?.includes(permission) ?? false;
}

function authHeaders(): Record<string, string> {
    const session = sessionStore.get();
    return session ? { Authorization: `Bearer ${session.token}` } : {};
}

async function call<T>(method: string, path: string, body?: unknown): Promise<T> {
    const response = await fetch(path, {
        method,
        headers: { Accept: 'application/json', ...(body !== undefined ? { 'Content-Type': 'application/json' } : {}), ...authHeaders() },
        body: body !== undefined ? JSON.stringify(body) : undefined,
    });
    if (!response.ok) throw new ApiError(response.status, await response.text().catch(() => ''));
    if (response.status === 204) return undefined as T;
    const envelope = (await response.json()) as { message: T | null };
    return envelope.message as T;
}

/** Заголовки входа для живого канала: он читается fetch'ем, а не EventSource, которому заголовок не передать. */
export function liveHeaders(): Record<string, string> {
    return { Accept: 'text/event-stream', ...authHeaders() };
}

/** Звук и картинки из шлюза с ключом входа: `<audio src>` и `<img src>` ключ не передают, поэтому — fetch в blob. */
async function fetchAudio(path: string): Promise<Blob> {
    const response = await fetch(path, { headers: authHeaders() });
    if (!response.ok) throw new ApiError(response.status, await response.text().catch(() => ''));
    return response.blob();
}

const get = <T>(path: string) => call<T>('GET', path);
const post = <T>(path: string, body: unknown = {}) => call<T>('POST', path, body);
const put = <T>(path: string, body: unknown) => call<T>('PUT', path, body);
const patch = <T>(path: string, body: unknown) => call<T>('PATCH', path, body);
const del = <T>(path: string) => call<T>('DELETE', path);

export type Executor = 'own' | 'yandex' | 'unsupported';

/** Режим колонки: Яндекс (как до шлюза), Шлюз, Свой (шлюз без Яндекса). */
export type PanelMode = 'relay' | 'gateway' | 'own';

export interface ModeView {
    mode: PanelMode;
    note: string;
    /** Чей голос выбран. */
    voice: 'yandex' | 'own';
    /** Чей звучит с режимом: в «Своём» голос Алисы уступает нашему. */
    speaking: 'yandex' | 'own';
    /** Кто выбрал режим: человек или сам шлюз (компьютер пропал). Шлюз постарше не присылает. */
    by?: 'person' | 'auto';
    /** Переход, который шлюз сделал сам: из какого режима, почему и когда. Вернётся сам, если человек не решит иначе. */
    auto?: { from: PanelMode; to: PanelMode; reason: string; at: string };
}

export type LlmEffort = 'low' | 'medium' | 'high' | 'xhigh' | 'max';

/** Модель для разговора — «мозг» дашборда: какая, два профиля, глубина мышления, уходит ли вопрос из дома. */
export interface BrainStatus {
    /** anthropic, openai (любой OpenAI-совместимый: Ollama, LM Studio, OpenRouter), echo для проверок, none. */
    provider: 'anthropic' | 'openai' | 'echo' | 'none';
    /** Профиль выбирает маршрутизатор на каждый вопрос; одна модель, когда оба одинаковы. */
    models: { fast: string; smart: string };
    /** Только у Claude: сколько думать до первого слова (LLM_EFFORT или выбор в пульте). */
    effort?: LlmEffort;
    cloud: boolean;
    /** Провайдер и модель каждого профиля; шлюз постарше их не присылает. */
    profiles?: { fast: BrainProfile | null; smart: BrainProfile | null };
    /** Откуда выбор: из пульта или из .env, пока в пульте ничего не выбрано. */
    source?: 'env' | 'panel';
}

export interface BrainProfile {
    provider: string;
    kind: 'anthropic' | 'openai' | 'echo' | 'none';
    title: string;
    model: string;
    cloud: boolean;
}

/** Готовый провайдер из набора шлюза или «свой». */
export interface ProviderPreset {
    id: string;
    title: string;
    description: string;
    kind: 'anthropic' | 'openai';
    baseUrl?: string;
    editableUrl: boolean;
    needsKey: boolean;
    keyUrl?: string;
    fields?: Array<{ id: string; title: string; placeholder?: string }>;
    suggested?: { fast: string; smart: string; more?: string[] };
    local?: boolean;
    brand?: string;
    order: number;
}

/** Последняя «Проверка» провайдера. */
export interface ProviderCheck {
    at: string;
    ok: boolean;
    model?: string;
    firstWordMs?: number;
    tools?: boolean;
    problem?: string;
}

/** Провайдер, добавленный в пульте: ключа нет ни в одном ответе — только задан ли и его хвост. */
export interface ProviderView {
    id: string;
    preset: string;
    kind: 'anthropic' | 'openai';
    title: string;
    baseUrl: string | null;
    options: Record<string, string>;
    key: { set: boolean; hint?: string };
    needsKey: boolean;
    editableUrl: boolean;
    models: string[];
    check: ProviderCheck | null;
    cloud: boolean;
    local: boolean;
    brand?: string;
}

/** Готовый MCP-сервер из набора шлюза. */
export interface McpPreset {
    id: string;
    title: string;
    description: string;
    brand?: string;
    transport: 'stdio' | 'http';
    command?: string;
    args?: string[];
    url?: string;
    auth: 'none' | 'token' | 'oauth';
    fields: Array<{ name: string; title: string; secret: boolean; placeholder?: string; url?: string }>;
    install?: { runner: 'npx' | 'uvx'; package: string };
    outside: boolean;
    capability?: string;
    note?: string;
    order: number;
    /** Можно подключить сейчас; иначе — почему нет (нет адреса Home Assistant). */
    ready: boolean;
    why?: string;
}

export type McpState = 'off' | 'connecting' | 'ready' | 'error' | 'login';

export interface McpTool {
    name: string;
    title: string;
    description: string;
    on: boolean;
    /** Сервер поменял описание после «включить»: выключен, пока не включите снова. */
    changed: boolean;
    readOnly: boolean;
    destructive: boolean;
    openWorld: boolean;
    hint?: string;
}

export interface McpServer {
    id: string;
    preset: string | null;
    title: string;
    brand?: string;
    transport: 'stdio' | 'http';
    command?: string;
    args?: string[];
    url?: string;
    auth: 'none' | 'token' | 'oauth';
    enabled: boolean;
    note: string;
    capability: string | null;
    voice: boolean;
    adminOnly: boolean;
    outside: boolean;
    cloud: boolean;
    state: McpState;
    detail?: string;
    server?: { name: string; version: string };
    secrets: Array<{ kind: 'env' | 'header' | 'token'; name: string; set: boolean; hint?: string }>;
    tools: McpTool[];
    instructions: { text?: string; approved: boolean };
    install?: { runner: 'npx' | 'uvx'; package: string };
    log: string[];
}

export interface McpView {
    presets: McpPreset[];
    servers: McpServer[];
    toolsOn: number;
    toolsLimit: number;
}

export interface McpServerInput {
    preset?: string;
    title?: string;
    transport?: 'stdio' | 'http';
    command?: string;
    args?: string[] | string;
    url?: string;
    auth?: 'none' | 'token' | 'oauth';
    /** Значения — секреты: пустая строка убирает, отсутствие — не меняет. */
    env?: Record<string, string>;
    headers?: Record<string, string>;
    token?: string;
    note?: string;
    capability?: string | null;
    voice?: boolean;
    adminOnly?: boolean;
    enabled?: boolean;
    outside?: boolean;
}

/** Событие проверочного чата «Спросить мозг». */
export type AskEvent =
    | { type: 'start'; model: string; provider: string; tools: number }
    | { type: 'text'; text: string; ms: number }
    | { type: 'tool'; name: string; server?: string; input: Record<string, unknown> }
    | { type: 'result'; name: string; content: string; isError: boolean; outside: boolean; ms: number }
    | { type: 'done'; ms: number }
    | { type: 'error'; message: string };

/** Вопрос модели из пульта: события приходят по мере ответа; abort — прервать. */
export async function askBrain(question: string, fresh: boolean, onEvent: (event: AskEvent) => void, signal?: AbortSignal): Promise<void> {
    const response = await fetch('/api/v1/brain/ask', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json', Accept: 'text/event-stream', ...authHeaders() },
        body: JSON.stringify({ question, fresh }),
        signal,
    });
    if (!response.ok || !response.body) throw new ApiError(response.status, await response.text().catch(() => ''));
    const reader = response.body.getReader();
    const decoder = new TextDecoder();
    let buffer = '';
    for (;;) {
        const { done, value } = await reader.read();
        if (done) break;
        buffer += decoder.decode(value, { stream: true });
        for (let end = buffer.indexOf('\n\n'); end >= 0; end = buffer.indexOf('\n\n')) {
            const chunk = buffer.slice(0, end);
            buffer = buffer.slice(end + 2);
            const data = chunk
                .split('\n')
                .filter((line) => line.startsWith('data: '))
                .map((line) => line.slice(6))
                .join('\n');
            if (data) onEvent(JSON.parse(data) as AskEvent);
        }
    }
}

/** Событие чата с агентом мастерской: его слова, инструменты, которые он зовёт, и что они вернули. */
export type WorkshopEvent =
    | { type: 'start'; model: string; provider: string }
    | { type: 'text'; text: string; ms: number }
    | { type: 'tool'; name: string; input: Record<string, unknown> }
    | { type: 'result'; name: string; content: string; isError: boolean; ms: number }
    | { type: 'done'; ms: number }
    | { type: 'error'; message: string };

/** Читает поток событий (server-sent events) ответа шлюза. */
async function readEvents<T>(response: Response, onEvent: (event: T) => void): Promise<void> {
    if (!response.ok || !response.body) throw new ApiError(response.status, await response.text().catch(() => ''));
    const reader = response.body.getReader();
    const decoder = new TextDecoder();
    let buffer = '';
    for (;;) {
        const { done, value } = await reader.read();
        if (done) break;
        buffer += decoder.decode(value, { stream: true });
        for (let end = buffer.indexOf('\n\n'); end >= 0; end = buffer.indexOf('\n\n')) {
            const chunk = buffer.slice(0, end);
            buffer = buffer.slice(end + 2);
            const data = chunk
                .split('\n')
                .filter((line) => line.startsWith('data: '))
                .map((line) => line.slice(6))
                .join('\n');
            if (data) onEvent(JSON.parse(data) as T);
        }
    }
}

/** Сообщение агенту мастерской о черновике: события приходят по мере ответа; abort — прервать. */
export async function workshopChat(draft: string, message: string, fresh: boolean, onEvent: (event: WorkshopEvent) => void, signal?: AbortSignal): Promise<void> {
    const response = await fetch(`/api/v1/workshop/drafts/${encodeURIComponent(draft)}/chat`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json', Accept: 'text/event-stream', ...authHeaders() },
        body: JSON.stringify({ message, fresh }),
        signal,
    });
    await readEvents(response, onEvent);
}

export interface BrainPick {
    provider: string;
    model: string;
}

/** «Мозг» в пульте: провайдеры, выбор моделей и модель из .env, которая отвечает, пока выбора нет. */
export interface BrainView {
    presets: ProviderPreset[];
    providers: ProviderView[];
    choice: { fast: BrainPick | null; smart: BrainPick | null; effort: LlmEffort };
    status: BrainStatus;
    env: { provider: BrainStatus['provider']; title: string; models: { fast: string; smart: string }; usable: boolean; cloud: boolean };
}

export interface ProviderInput {
    preset?: string;
    title?: string;
    baseUrl?: string;
    /** Не передан — ключ прежний; пустая строка — убрать. */
    key?: string;
    options?: Record<string, string>;
}

/**
 * Что это за сборка шлюза (только у шлюза на Rust): версия, сборка ли для самой колонки и что в ней есть. Что где
 * доступно сейчас — не отсюда, а из списка шлюза (availability.ts).
 */
export interface BuildInfo {
    version: string;
    station: boolean;
    /** models, voiceLab, calls, stationConnect, mcpPrograms */
    features: string[];
    /** Режимы, которые можно включить сейчас. Шлюз постарше не присылает — тогда все три. */
    modes?: PanelMode[];
    /** Версия API пары «компьютер — колонка». */
    peerApi?: number;
}

/** Состояние шлюза из его базы: кто выбрал режим, страховка, где главные настройки. */
export interface GatewayState {
    modeBy: 'person' | 'auto';
    auto?: { from: PanelMode; to: PanelMode; reason: string; at: string };
    safety: boolean;
    safetyLast?: { what: string; at: string };
    settingsHome: 'station' | 'pc';
}

/** Что компьютер знает о шлюзе на колонке. */
export interface StationPlacementRecord {
    placement: Placement;
    address: string;
    port: number;
    device?: string;
    settingsHome: 'station' | 'pc';
    autoUpdate: boolean;
    managed: boolean;
    homeKeyId?: string;
    movedAt?: string;
}

/** Размещение колонки в /health компьютера. */
export interface PlacedStation {
    key: string;
    device?: string;
    placement: Placement;
    address: string;
    port?: number;
    reachable: boolean;
    seenAt?: string;
    version?: string;
}

/** Колонка, о которой запрос (`?station=`); без неё — единственная с размещением на компьютере. */
const forStation = (path: string, station?: string) =>
    station ? `${path}?station=${encodeURIComponent(station)}` : path;

export interface StationGatewayView {
    /** Какая колонка: её устройство, или «at:<адрес>» — шлюз, связанный вручную. */
    key: string;
    record?: StationPlacementRecord;
    live: {
        reachable: boolean;
        checkedAt?: string;
        seenAt?: string;
        error?: string;
        health?: { build?: { version?: string; peerApi?: number }; station?: { connected?: boolean }; placement?: PlacementHealth };
        sentRevision?: number;
        sentAt?: string;
        syncError?: string;
        updating: boolean;
        updateError?: string;
    };
    version?: string;
    compatible: boolean;
    update?: string;
    payload: { source: 'builtIn' | 'file' | 'feed' | 'none'; version: string; note: string };
    backups: Array<{ id: string; at: string; revision: number; size: number }>;
    warnings: Array<{ kind: 'silent' | 'voiceAway' | 'safety'; text: string }>;
    copyStale: boolean;
    stationBehind: boolean;
}

export type MoveTo = 'station' | 'stationServer' | 'pc' | 'yandex';
export type MoveStepId = 'check' | 'copy' | 'install' | 'settings' | 'point' | 'arrive' | 'stop';

export interface MoveJob {
    id: string;
    to: MoveTo;
    address: string;
    status: 'running' | 'done' | 'failed';
    steps: Array<{ id: MoveStepId; state: 'pending' | 'running' | 'done' | 'failed' | 'skipped' }>;
    error?: string;
    note?: string;
    startedAt: string;
}

/** «Где работает шлюз» для обоих пультов. */
export interface PlacementView {
    role: 'pc' | 'station';
    placement: Placement;
    peerApi: number;
    state: GatewayState;
    health: PlacementHealth;
    managedBy?: string;
    station?: StationGatewayView;
    /** Компьютер: каждая колонка, у которой здесь есть размещение. */
    stations?: StationGatewayView[];
    job?: MoveJob;
    homeAddresses?: string[];
}

/** Домашний сервер колонки: адрес компьютера, ключ, на связи ли он. */
export interface HomeServerStatus {
    /** Адрес и ключ есть. */
    configured: boolean;
    /** «ПК помогает колонке»: колонка опирается на ПК. Шлюз постарше поля не шлёт — тогда как configured. */
    enabled?: boolean;
    url?: string;
    key: { set: boolean; hint?: string };
    state: 'none' | 'up' | 'down';
    since?: string;
    error?: string;
}

/** Где работает шлюз (/health → placement): кто он, размещение, а у шлюза на колонке — место, память, сервер. */
export interface PlacementHealth {
    role: 'pc' | 'station';
    placement: Placement;
    peerApi: number;
    settingsRevision?: number;
    data?: { freeKb: number; totalKb: number };
    memory?: { rssKb?: number; totalKb?: number; availableKb?: number };
    homeServer?: { url: string; enabled?: boolean; state: 'none' | 'up' | 'down'; since?: string; error?: string };
    voice?: { url: string; pointsTo: 'yandex' | 'gateway' | 'other'; local?: boolean };
    managedBy?: string;
    settingsHome?: 'station' | 'pc';
    /** Колонка: когда компьютер последний раз взял копию её настроек и когда прислал свои. */
    settingsCopiedAt?: string;
    settingsReceivedAt?: string;
    safety?: boolean;
    safetyLast?: { what: string; at: string };
    /** У компьютера: шлюз на колонке, каким его видно. */
    station?: PlacedStation;
    /** Каждая колонка с размещением на этом компьютере. */
    stations?: PlacedStation[];
}

export interface Health {
    status: string;
    build?: BuildInfo;
    /** gateway: отвечаем мы; relay: сессия колонки идёт в Яндекс, мы только журналируем. */
    mode?: 'gateway' | 'relay';
    panelMode?: PanelMode;
    activeSessions?: number;
    speech?: { vad: string; asr: string };
    dialog?: string;
    /** Шлюз постарше его не присылает: тогда мозг на дашборде — «—». */
    brain?: BrainStatus;
    tts?: string;
    station?: { connected: boolean; lastSeenAt: string };
    placement?: PlacementHealth;
    skills?: {
        routes: Record<string, string>;
        implemented: Record<string, boolean>;
        served: Record<string, Record<Executor, number>>;
        unsupported: Record<string, number>;
    };
}

export interface SessionSnapshot {
    id: string;
    mode?: 'gateway' | 'relay';
    deviceTag: string;
    remoteAddress: string;
    connectedAt: string;
    closedAt?: string;
    device?: {
        deviceModel?: string;
        firmwareVersion?: string;
        timezone?: string;
        lang?: string;
        platform?: string;
    };
    activeVoiceRequests: number;
    canSpeak: boolean;
    yandexConnected?: boolean;
    events: number;
}

/** Шаги пути вопроса: слух слушает и распознаёт, мозг думает и наводит справки, голос отвечает. */
export type RequestPhase = 'listen' | 'recognize' | 'think' | 'tool' | 'speak' | 'play';
export type RequestStatus = 'active' | 'answered' | 'silent' | 'cancelled' | 'failed';
/** Кто сделал шаг: дома, облако, Яндекс, помощник. */
export type Place = 'own' | 'cloud' | 'yandex' | 'agent';

export interface RequestStep {
    phase: RequestPhase;
    /** мс от начала запроса. */
    at: number;
    /** Сколько шёл; нет — идёт сейчас. */
    ms?: number;
    by?: Place;
    /** Модель, голос или возможность: «gigaam-v3», «claude-haiku-5-5», «Погода». */
    what?: string;
}

/**
 * Запрос к колонке целиком: от первого звука до последнего слова ответа (gateway/src/core/requests.rs на шлюзе). Слова —
 * text, partial, answer, error — приходят только тому, у кого есть право на журнал.
 */
export interface RequestView {
    id: string;
    device: string;
    session?: string;
    kind: 'voice' | 'text' | 'say';
    mode: string;
    status: RequestStatus;
    phase?: RequestPhase;
    startedAt: string;
    ms?: number;
    text?: string;
    partial?: string;
    answer?: string;
    capability?: string;
    /** Кто ответил: own — наш навык, model — модель, yandex, unsupported — пока никто. */
    executor?: string;
    steps: RequestStep[];
    error?: string;
    parent?: string;
}

/** Как колонку слышно: голосовой канал, её собственные клиенты (микрофон, звонки) или никак. */
export interface StationPresence {
    device: string;
    state: 'voice' | 'alive' | 'reconnecting' | 'offline';
    channels: Array<'voice' | 'microphone' | 'calls'>;
    lastSeen?: string;
    since: string;
}

/** Живой канал шлюза (/api/v1/live): сначала состояние, потом каждое изменение. */
export type LiveMessage =
    | { type: 'hello'; serverTime: string; presence: StationPresence[]; active: RequestView[]; recent: RequestView[] }
    | { type: 'request'; request: RequestView }
    | { type: 'presence'; presence: StationPresence }
    | { type: 'changed'; name: string };

export interface UnsupportedEntry {
    at: string;
    device: string;
    capability: string;
    text: string;
    route: string;
    reason: string;
}

export interface RoutesSetting {
    routes: Record<string, string>;
    /** false — кто-то уже менял маршруты из пульта; true — ещё дефолт из ROUTES. */
    isDefault: boolean;
}

/** Магазин моделей: карточки каталога с реальностью поверх. */
export interface AsrCard {
    id: string;
    model: string;
    title: string;
    description: string;
    badge?: string;
    license?: string;
    /** Советуем: показывается в панели «Советуем» (GigaAM v3). */
    recommended?: boolean;
    active: boolean;
    pending: boolean;
}

export interface TtsCard {
    id: string;
    engine: 'yandex' | 'silero' | 'piper' | CloneEngineId;
    /** own — свой голос (отпечаток, «Свои голоса»): в списке сразу после рекомендованных. */
    group?: 'own';
    /** builtin — рекомендованная карточка витрины; registry — открытый реестр Piper. */
    source?: 'builtin' | 'registry';
    /** Про свой голос: его id в лаборатории, когда снят, образец, пробы, отвечает ли его сервер. */
    own?: { id: string; createdAt: string; refSeconds?: number; samples: number; engineTitle: string; serving: boolean };
    title: string;
    description: string;
    badge?: string;
    license?: string;
    voice?: string;
    size?: string;
    /** Язык голоса: 'ru_RU' у пайпера, 'ru' у закреплённых. */
    locale?: string;
    /** Тяжесть нейросети пайпера: x_low, low, medium, high. */
    quality?: string;
    /** Голоса внутри модели (у силеро пять, у пайпера один) — чем говорит синтез. */
    voices: Array<{ id: string; name: string }>;
    /** Все файлы на диске (у пайпера и его .onnx.json). */
    installed: boolean;
    /** Файл на диске и голосовой сервер его загрузил — только такой голос правда звучит. */
    loaded: boolean;
    active: boolean;
    /** Скачивается отсюда (файлы Silero кладут вручную). */
    downloadable: boolean;
    /** Готов ли голос здесь: нужна модель или файл, ставится, не пойдёт на этом компьютере. Шлюз постарше не присылает. */
    state?: VoiceState;
    /** Ход установки модели или файла, 0–100 — пока state: installing. */
    progress?: number;
    /** Почему не пойдёт (state: unsupported) или что не так. */
    reason?: string | null;
    /** Чего голосу не хватает: модель или файл. */
    needs?: VoiceNeeds | null;
}

/** Готов ли голос: ready — звучит; needs — нужна модель или файл; installing — ставится; unsupported — не пойдёт здесь. */
export type VoiceState = 'ready' | 'needs' | 'installing' | 'unsupported';

/**
 * Чего не хватает голосу или пакету: модель голоса (engine) или файл голоса (file, Piper и Silero). Один язык у
 * карточек голоса, у персонажей и у каталога пакетов (docs/plans/characters.md, раздел 5).
 */
export interface VoiceNeeds {
    kind: 'engine' | 'file';
    id: string;
    title?: string;
    downloadMb?: number;
    diskMb?: number;
    minutes?: number;
    /** В каталоге: пакет работает и без этого (персонаж говорит запасным голосом). */
    optional?: boolean;
    /** Свой голос, снятый другой моделью: есть записи и та модель стоит — его можно переснять. */
    alternatives?: Array<{ kind: 'reimprint'; engine: string; title?: string }>;
}

/**
 * Отложенный выбор голоса (настройка voice_pending): «Скачать и включить» поставил модель или файл, голос включится,
 * когда они встанут. tight — модель встала, но памяти впритык: ждёт «всё равно»; failed — установка не удалась.
 */
export interface TtsPending {
    modelId: string;
    voiceId?: string | null;
    title?: string | null;
    /** waiting — ждёт модель, файл или снимок; tight — памяти впритык; failed — не вышло. */
    state: 'waiting' | 'installing' | 'tight' | 'failed' | string;
    /** Чего ждёт: модели (engine), файла голоса (file) или снимка голоса (imprint). */
    waitFor?: { kind: 'engine' | 'file' | 'imprint' | string; id: string } | null;
    progress?: number;
    reason?: string | null;
    needs?: VoiceNeeds | null;
    since?: string;
}

export interface DownloadState {
    id: string;
    state: 'downloading' | 'done' | 'error' | 'canceled';
    bytes: number;
    total: number;
    error?: string;
}

export interface ModelsView {
    asr: AsrCard[];
    tts: TtsCard[];
    /** Что говорит сейчас, по id каталога; modelId 'yandex' — голос Алисы. */
    ttsActive: { modelId: string; voiceId: string };
    /** Чей голос выбран — о нём предупреждает «Свой». */
    voice: 'yandex' | 'own';
    /** Чей голос звучит с режимом. */
    speaking: 'yandex' | 'own';
    mode: PanelMode;
    asrLoaded: string | null;
    /** Голосовой сервер отвечает. */
    ttsUp: boolean;
    downloads: DownloadState[];
    canRestart: boolean;
    ownVoiceServer: boolean;
    restarting: 'asr' | 'tts' | null;
    /** Голос, который включится, когда встанет его модель или файл; нет — ничего не ждёт. */
    ttsPending?: TtsPending | null;
}

/** Модели, которыми снимают отпечаток голоса. */
export type CloneEngineId = 'omnivoice' | 'cosyvoice3' | 'voxcpm';

export interface VoiceLabEngine {
    id: CloneEngineId;
    title: string;
    about: string;
    license: string;
    recommended: boolean;
    status: 'ready' | 'installing' | 'missing' | 'unsupported';
    /** Где здесь звучит её голос: видеокарта Мака, NVIDIA или процессор. */
    device?: 'mps' | 'cuda' | 'cpu';
    slow?: boolean;
    reason?: string;
    diskGb: number;
    /** Сколько памяти нужно отпечатку и серверу голоса здесь, МБ (замер прошлого раза или оценка). */
    needMb: { imprint: number; serve: number };
    ready?: { device: string; source: 'bench' | 'download' };
}

export interface VoiceLabTake {
    n: number;
    seconds: number;
    levelDb: number;
    noiseDb: number;
    snrDb: number;
    peakDb: number;
    clipping: number;
    verdict: 'good' | 'short' | 'quiet' | 'noisy' | 'clipped';
    passage?: number;
    at: string;
}

export interface OwnVoice {
    id: string;
    name: string;
    engine: CloneEngineId;
    device?: string;
    createdAt: string;
    refSeconds?: number;
    snrDb?: number;
    rtf?: number;
    samples: Array<{ n: number; text: string; seconds: number }>;
    takes: number;
    chosen: boolean;
}

export interface VoiceLabJob {
    id: string;
    kind: 'install' | 'imprint';
    engine: string;
    state: 'running' | 'done' | 'error' | 'stopped';
    stage: string;
    label: string;
    percent: number;
    log: string[];
    memoryMb?: number;
    peakMb?: number;
    limitMb?: number;
    error?: string;
    reason?: string;
    startedAt: string;
    finishedAt?: string;
    result?: { voiceId?: string; device?: string; peakMb?: number; rtf?: number };
}

export interface VoiceLabView {
    available: boolean;
    reason?: string;
    platform?: { summary: string; os: string; gpu: 'apple' | 'nvidia' | 'none'; uv: boolean; checkedAt: string };
    engines: VoiceLabEngine[];
    draft: { takes: VoiceLabTake[]; seconds: number };
    voices: OwnVoice[];
    job?: VoiceLabJob;
    /** Память Мака: впритык, когда свободное минус запас колонки меньше, чем нужно модели. */
    memory: {
        totalMb: number;
        poolMb: number;
        reserveMb: number;
        pressure: 'normal' | 'warn' | 'critical';
    };
    servers: Partial<
        Record<CloneEngineId, { up: boolean; starting: boolean; device?: string; error?: string; memoryMb?: number; peakMb?: number }>
    >;
    passages: string[];
}

/** Карточка характера: что на ней написано и какой текст она применяет. */
export interface PersonaPreset {
    id: string;
    title: string;
    description: string;
    badge?: string;
    text: string;
}

/** Характер Алисы: текст, которым говорит сейчас, и карточки на выбор. */
export interface PersonaInfo {
    persona: string;
    /** id карточки, чей текст говорит; null — свой характер. */
    activePreset: string | null;
    source: 'panel' | 'file' | 'default';
    presets: PersonaPreset[];
}

/** Вид пакета: плагин (код в шлюзе), приложение (своя программа рядом), персонаж и тема (данные без кода). */
export type PackageKind = 'plugin' | 'app' | 'character' | 'theme';

/** Пакет, который стоит на шлюзе (GET /api/v1/packages), любого вида. */
export interface PackageView {
    kind?: PackageKind | null;
    id: string;
    title: string;
    description?: string | null;
    /** Версия, что работает, или новейшая найденная, пока пакет выключен. */
    version?: string | null;
    versions: Array<{ version: string; problems: string[] }>;
    enabled: boolean;
    /** off, ready, loaded, error, safeMode — для плагинов. */
    state: string;
    loaded: boolean;
    problem?: string | null;
    permissions: string[];
    runs: string[];
    calls: number;
    errors: number;
    lastError?: { at: string; message: string } | null;
    lastCallAt?: string | null;
    /** Откуда: из каталога или поставлен руками. Шлюз постарше не присылает. */
    source?: 'catalog' | 'local' | 'bundled' | 'system';
    /** Проверен нами: всё из нашего каталога; поставленное руками и из чужих каталогов — нет. */
    verified?: boolean;
    license?: string | null;
    /** Откуда пакет — от этого зависят кнопки: системный не выключить, встроенный не удалить, свой — всё. */
    origin?: PackageOrigin;
    /** Плагин с prefer: home на колонке с сервером: где он работает сейчас. */
    runsAt?: 'home' | 'here' | null;
    removable?: boolean;
    canDisable?: boolean;
    editable?: boolean;
    /** Его часть пульта, пока он включён. */
    panelModule?: string | null;
    panelStyles?: string | null;
    /** Поля настроек из манифеста со значениями: пульт рисует форму сам. */
    settings?: Array<{ key: string; type: 'string' | 'number' | 'boolean'; title: string; description?: string; default?: unknown; options?: string[]; value?: unknown }>;
    /** Что плагин даёт шлюзу. */
    contributes?: { phrases: string[]; tools: string[]; media: string[]; events: string[]; panel: boolean; themes: boolean };
    /** Сколько плагин просит: память — это лимит, место — его хранилище. */
    budget?: { memoryMb: number; storageMb: number } | null;
    /** Пока код загружен: сколько держит движок, байты. */
    memory?: { allocated: number; used: number; limit: number } | null;
    /** Его хранилище kv, байты. */
    storageUsed?: number;
}

/** Вид пакета по тому, откуда он: системный, встроенный (в сборке шлюза), свой, из каталога. Ставит шлюз. */
export type PackageOrigin = 'system' | 'bundled' | 'user' | 'catalog';

/** Часть пульта включённого плагина: что загрузить (GET /api/v1/packages/panel). */
export interface PanelPart {
    id: string;
    title: string;
    version: string;
    module?: string | null;
    styles?: string | null;
    settings?: Record<string, unknown>;
}

/** Черновик мастерской (DATA_DIR/packages-dev/<id>). */
export interface DraftView {
    id: string;
    kind?: PackageKind | null;
    title?: string | null;
    version?: string | null;
    files: Array<{ path: string; size: number }>;
    /** Манифест не читается — почему. */
    problems: string[];
    /** Что стоит на шлюзе под этим id и откуда. */
    installed?: string | null;
    origin?: PackageOrigin | null;
    /** Проба черновика на шлюзе идёт сейчас: какая версия, вместо какой и до какого времени. */
    trial?: DraftTrial | null;
}

/** Проба на колонке: черновик работает на шлюзе до `until`, потом возвращается `previous` (или пакет уходит). */
export interface DraftTrial {
    version: string;
    previous?: string | null;
    wasEnabled: boolean;
    until: string;
}

/** Что сделает «Поставить» (GET …/install): версии, права и хосты, что даёт черновик. */
export interface InstallPlan {
    kind: PackageKind;
    title: string;
    from?: string | null;
    to: string;
    first: boolean;
    added: string[];
    removed: string[];
    hostsAdded: string[];
    hostsRemoved: string[];
    kept: string[];
    budget: { memoryMb: number; storageMb: number };
    phrases: string[];
    tools: string[];
    enabled: boolean;
    keptVersions: number;
}

/** Модель агента мастерской: свой выбор (null — умный профиль «Мозга») и кто отвечает сейчас. */
export interface WorkshopAgentModel {
    choice: BrainPick | null;
    answers: BrainProfile | null;
    smart: BrainProfile | null;
}

export interface WorkshopView {
    dir: string;
    drafts: DraftView[];
}

/** Строка журнала плагина: его console, вызовы, загрузки, что колонка сказала о его станциях. */
export interface JournalLine {
    at: string;
    level: string;
    message: string;
}

export interface CheckReport {
    ok: boolean;
    kind?: PackageKind | null;
    version?: string | null;
    manifest: string[];
    code?: string | null;
    exports: string[];
    missing: string[];
    warnings: string[];
    ms: number;
}

export interface RunReport {
    ok: boolean;
    result: unknown;
    error?: string | null;
    log: JournalLine[];
    said: string[];
    kv: Record<string, unknown>;
    ms: number;
}

export interface PhraseReport {
    text: string;
    normalized: string;
    matched?: string | null;
    handler?: string | null;
    capability?: string | null;
    slots: Record<string, unknown>;
    answer: unknown;
    speech?: string | null;
    play: Array<{ title: string; url: string; format: string; type: string }>;
    listen: boolean;
    letGo: boolean;
    error?: string | null;
    warnings: string[];
    log: JournalLine[];
    said: string[];
    ms: number;
}

/** Что администратор одобряет перед установкой: первую установку или новые права и хосты. */
export interface Approval {
    first: boolean;
    permissions: string[];
    hosts: string[];
    budget: { memoryMb: number; storageMb: number };
}

export interface InstallReport {
    installed: boolean;
    version?: string | null;
    needsApproval?: Approval | null;
    message: string;
    package?: PackageView | null;
}

/** Плагин в счёте памяти: сколько объявил и сколько держит сейчас. */
export interface PluginShare {
    id: string;
    title: string;
    enabled: boolean;
    loaded: boolean;
    budgetMb: number;
    actualMb?: number | null;
}

/** Память и место плагинов (GET /api/v1/packages/resources): «Память колонки». */
export interface PackagesResources {
    placement: Placement;
    /** На колонке бюджет соблюдается: не помещается — плагин не включится. На компьютере — только показ. */
    enforced: boolean;
    measured: boolean;
    totalMb: number;
    availableMb: number;
    reserveMb: number;
    gatewayMb?: number | null;
    pluginsMb: number;
    budgetMb: number;
    committedMb: number;
    freeMb: number;
    pressure: 'normal' | 'warn' | 'critical';
    deferring: boolean;
    action: 'disable' | 'warn';
    disk: { freeMb?: number | null; reserveMb: number; packagesMb: number };
    plugins: PluginShare[];
    note: string;
}

export interface PackagesView {
    /** PLUGINS_DISABLED: пакеты видны, ни один плагин не работает. */
    safeMode: boolean;
    dir: string;
    placement: Placement;
    idleUnloadS: number;
    packages: PackageView[];
}

/** Что с пакетом каталога здесь: можно поставить, стоит, есть новее, не пойдёт, нужна подпись (плагины, этап 8). */
export type CatalogState = 'available' | 'installed' | 'update' | 'incompatible' | 'needsSignature';

/** Пакет из ленты каталога с его состоянием на этом шлюзе. */
export interface CatalogEntry {
    id: string;
    kind: PackageKind;
    version: string;
    title: string;
    description?: string | null;
    license?: string | null;
    verified?: boolean;
    runs?: string[];
    minGateway?: string | null;
    /** Что пакету понадобится: модель голоса, файл. */
    needs?: VoiceNeeds[] | null;
    /** Адреса шлюза: картинка и фразы-пробники, слышно до установки. */
    preview?: { image?: string | null; samples?: string[] } | null;
    /** Стоит ли: версия, что стоит (или true); нет — не стоит. */
    installed?: string | boolean | null;
    state: CatalogState;
    /** Почему нельзя поставить. */
    reason?: string | null;
}

/** Каталог пакетов (GET /api/v1/packages/catalog): лента с состоянием каждого; без сети — последняя копия. */
export interface CatalogView {
    /** Лента подписана ключом каталога: из неё ставятся и плагины. */
    signed?: boolean;
    /** У этой сборки есть ключ каталога. */
    keyed?: boolean;
    /** Есть адрес каталога (вшитый или PACKAGES_FEED_URL). */
    configured: boolean;
    /** Выключатель «Не загружать каталог» не стоит. */
    enabled: boolean;
    url?: string | null;
    fetchedAt?: string | null;
    /** Сети нет — показана последняя копия (или ничего). */
    offline: boolean;
    problem?: string | null;
    packages: CatalogEntry[];
}

/** Голос персонажа: готов, нужна модель, ставится, не пойдёт здесь, говорит запасной или голоса нет вовсе. */
export interface CharacterVoice {
    state: VoiceState | 'fallback' | 'none';
    /** Модель, под которую снят голос персонажа здесь. */
    engine?: string | null;
    /** Карточка голоса в «Голосе». */
    cardId?: string | null;
    /** Запасной голос: им персонаж говорит, пока своего нет. */
    fallback?: { cardId: string; title: string } | null;
    needs?: VoiceNeeds | null;
    /** Ход установки модели, 0–100 (если шлюз его знает). */
    progress?: number;
    reason?: string | null;
}

/** Регулятор тона: каждое положение подставляет в промт одну строку. */
export interface CharacterTone {
    id: string;
    title: string;
    /** Положение по умолчанию — номер в options. */
    default: number;
    options: Array<{ label: string }>;
}

/** Персонаж: встроенный характер или персонаж из пакета (`<пакет>:<персонаж>`). */
export interface CharacterView {
    id: string;
    title: string;
    description: string;
    badge?: string | null;
    builtin: boolean;
    package?: { id: string; version: string; license?: string | null; verified: boolean } | null;
    gender?: string | null;
    /** Цвет персонажа, #RRGGBB. */
    color?: string | null;
    /** Адрес аватара на шлюзе. */
    avatar?: string | null;
    samples: Array<{ text: string; url: string }>;
    tones: CharacterTone[];
    voice: CharacterVoice;
    /** Персонаж из пакета не разобрался: почему. */
    problem?: string | null;
}

/** Кто говорит сейчас: id персонажа (null — свой текст), правили ли текст руками, есть ли в пакете новый. */
export interface ActiveCharacter {
    id: string | null;
    source?: string | null;
    edited: boolean;
    /** В пакете новый текст этого персонажа: true или сам текст (шлюз может прислать и его). */
    update?: boolean | string | { text?: string; version?: string } | null;
    /** Положения регуляторов: id тона → номер положения. */
    tones?: Record<string, number> | null;
    /** Карточку, выбранную раньше, заменил персонаж каталога (Мажордом → `arthur`): какой пакет предложить. */
    replacedBy?: string | null;
}

export interface CharactersView {
    active: ActiveCharacter | null;
    characters: CharacterView[];
}

/** Что выбрать вместе с персонажем: тоны, поставить ли модель голоса, «всё равно» при памяти впритык, менять ли голос. */
export interface ChooseInput {
    tones?: Record<string, number>;
    install?: boolean;
    anyway?: boolean;
    /** false — только характер, голос остаётся прежним. */
    voice?: boolean;
}

/** Ответ выбора: характер уже стоит; голос — готов, нужна модель (state: needs) или ждёт её (pending). */
export interface ChooseResult {
    character?: CharacterView | string | null;
    voice?: { state: string; needs?: VoiceNeeds | null; pending?: TtsPending | null } | null;
}

/** Человек дома: с паролем — входит в пульт; без — профиль, который знает колонка. */
export interface Person {
    id: string;
    name: string;
    role: Role;
    status: 'active' | 'disabled';
    canSignIn: boolean;
    restrictions: { allow?: string[]; deny?: string[] };
    profile: { color?: string; aliases?: string[] };
    lastLoginAt: string | null;
    createdAt: string;
}

/** Роли, права и возможности: из чего выбирают редакторы пульта. Источник — шлюз. */
export interface AccessCatalog {
    roles: Array<{ id: Role; title: string; description: string; grants: string[] }>;
    permissions: Array<{ id: string; title: string }>;
    capabilities: Array<{ id: string; title: string; permission: string }>;
}

export interface Me {
    id: string;
    kind: 'user' | 'apiKey' | 'household' | 'system';
    name: string;
    role?: Role;
    permissions: string[];
}

export interface ApiKey {
    id: string;
    name: string;
    grants: string[];
    createdAt: string;
    lastUsedAt: string | null;
    expiresAt: string | null;
    revoked: boolean;
}

/** Вариант модели: «Лайт 2 с часами» и цвета, в которых он продаётся (первый — по умолчанию). */
export interface LookVariant {
    id: string;
    title: string;
    /** Слово на сегменте выбора: «С часами». */
    label: string;
    clock: boolean;
    colors: string[];
}

/** Откуда часть вида: колонка сообщила сама, выбрал человек, или стоит по умолчанию. */
export type LookSource = 'station' | 'chosen' | 'default';

/** Вид колонки по решению шлюза: что ясно из её данных, что выбрал человек и что можно выбрать ещё. */
export interface StationLook {
    family: string;
    title: string;
    model: string;
    color: string;
    clock: boolean;
    modelFrom: LookSource;
    colorFrom: LookSource;
    /** Варианты, которые данные оставляют открытыми (один — решено). */
    variants: LookVariant[];
    /** Цвета варианта; один, если цвет колонка сообщила сама. */
    colors: string[];
    /** Наклейка-котик на фасаде: null — у модели наклеек не бывает (они есть только у Лайт 2); шлюз постарше не присылает. */
    sticker?: boolean | null;
}

export interface Station {
    device: string;
    name: string;
    status: 'approved' | 'pending' | 'blocked';
    lastAddress: string | null;
    firstSeen: string;
    lastSeen: string;
    /** Что колонка сказала о себе при синхронизации; шлюз постарше этих полей не присылает. */
    deviceModel?: string | null;
    deviceColor?: string | null;
    /** Прошивка, названная при последней синхронизации, и проверяли ли с ней шлюз (нет поля — о модели не знаем). */
    firmware?: string | null;
    firmwareTried?: boolean;
    /** Выбор человека; null — по умолчанию. */
    model?: string | null;
    color?: string | null;
    /** null — для этой модели рисунка нет. */
    look?: StationLook | null;
}

/** Работает · настроено (не проверено) · не настроено · выключено · скоро (нет в этой сборке) · нет связи. */
export type ConnectorState = 'ready' | 'configured' | 'unset' | 'off' | 'soon' | 'error';

export interface ConnectorView {
    manifest: {
        id: string;
        kind: string;
        title: string;
        description: string;
        cloud: boolean;
        env: string[];
        capabilities?: string[];
        /** Где настраивается: свой лист здесь, «Мозг», «Голос» или режим на дашборде. */
        setup?: 'panel' | 'brain' | 'voice' | 'mode';
    };
    status: { state: ConnectorState; detail?: string };
}

/** Итог «Проверить»: словами, у новостей — по каждой ленте. */
export interface ConnectorCheck {
    ok: boolean;
    text: string;
    at: string;
    items?: Array<{ key: string; ok: boolean; text: string }>;
}

/** Контакт подключения: имя и последние цифры номера (сам номер пульт не видит). */
export interface ContactView {
    name: string;
    hint: string;
}

export interface NewsFeedEntry {
    url: string;
    title?: string;
    topic?: string;
    catalog?: string;
    enabled: boolean;
}

/** Настройки подключения из «Коннекторов»: поля, маски секретов, откуда взялись, итог проверки. */
export interface ConnectorSettings {
    id: string;
    title: string;
    values: Record<string, unknown>;
    secrets: Record<string, { set: boolean; hint?: string }>;
    /** Настроено хоть раз (здесь или перенесено из .env). */
    stored: boolean;
    /** Всё ещё значения, перенесённые из .env. */
    fromEnv: boolean;
    /** Строки .env, которые шлюз больше не читает: их можно убрать. */
    envLeft: string[];
    status: ConnectorView['status'];
    check?: ConnectorCheck;
    /** Есть в этой сборке (звонков в сборке для колонки нет). */
    available: boolean;
    /** Сейчас идёт звонок через этот мост: сохранение его прервёт. */
    busy: boolean;
}

/** Кто написал боту и не в контактах: имя и последние цифры чата. */
export interface TelegramStranger {
    id: string;
    name: string;
    hint: string;
    at: string;
}

export interface NewsTopic {
    id: string;
    title: string;
    words: string[];
}

export interface CatalogFeed {
    id: string;
    title: string;
    site: string;
    url: string;
    topic: string;
}

export interface NewsCatalog {
    topics: NewsTopic[];
    feeds: CatalogFeed[];
}

export interface FeedPreview {
    url: string;
    title?: string;
    headlines: string[];
    count: number;
}

export interface AuditEntry {
    id: number;
    at: string;
    action: string;
    actor: { kind: string | null; id: string | null; name: string | null } | null;
    details: Record<string, unknown>;
}

/** Колонка, которую шлюз нашёл в сети (mDNS), — с её SSH, если он открыт. */
/** Колонка ждёт шлюз по прежнему адресу (was), а шлюз теперь на now: «Переподключить». */
export interface MovedGateway {
    address: string;
    device?: string;
    was: string;
    now: string;
}

export interface FoundStation {
    address: string;
    name?: string;
    txt: Record<string, string>;
    /** Программа SSH (`dropbear_2022.83`) или null: SSH закрыт. */
    ssh?: string | null;
}

export interface VoiceAddresses {
    url: string;
    quic: string;
}

/** Первый взгляд на колонку — только чтение. */
export interface StationCheck {
    address: string;
    ssh: string | null;
    fingerprint?: string;
    facts?: { addresses: VoiceAddresses; deviceType?: string; softwareVersion?: string; pointsTo: 'yandex' | 'gateway' | 'other' };
    dataFreeKb?: number;
    arch?: string;
    client?: boolean;
    updates?: 'blocked' | 'on';
    /**
     * Наша копия quasar.cfg на колонке: mounted — подключена поверх файла прошивки; copy — есть, но при последней
     * загрузке не подключилась (refused: firmware-changed — прошивка сменила свой файл, то есть обновилась; gateway-off
     * — шлюз на колонке выключен); none — нет.
     */
    copy?: { copy: 'mounted' | 'copy' | 'none'; refused?: string; refusedAt?: number };
    device?: string;
    target?: VoiceAddresses;
    connected?: boolean;
    /** no-ssh, no-config, host-key-changed, auth-failed, unreachable — или текст ошибки. */
    problem?: string;
}

export type ConnectStepId = 'copy' | 'write' | 'verify' | 'reboot' | 'arrive';

/** Подключение или откат колонки: шаги, которые пульт показывает галочками. */
export interface ConnectJob {
    id: string;
    kind: 'connect' | 'restore';
    address: string;
    status: 'running' | 'done' | 'failed' | 'timeout';
    steps: Array<{ id: ConnectStepId; state: 'pending' | 'running' | 'done' | 'failed' | 'skipped' }>;
    copy?: string;
    device?: string;
    error?: string;
    startedAt: string;
}

/** Обновления шлюза (только у шлюза на Rust): что стоит, что предлагает лента выпусков и кто ставит обновление. */
export interface UpdateStatus {
    current: string;
    platform: string;
    /** Есть ли лента выпусков, которую шлюз проверяет. */
    feed: boolean;
    checkedAt?: string;
    available: { version: string; notes?: string; date?: string } | null;
    installing: boolean;
    error?: string;
    /** desktop, docker, home-assistant: кто ставит обновления этого шлюза. */
    managedBy?: string;
    canInstall: boolean;
    /** Что сделать вместо «Обновить», если шлюз не ставит обновление сам. */
    hint?: string;
}

export const api = {
    health: () => get<Health>('/health'),
    // Колонка
    sessions: () => get<SessionSnapshot[]>('/api/v1/station/sessions'),
    unsupported: (limit = 30) => get<UnsupportedEntry[]>(`/api/v1/station/unsupported?limit=${limit}`),
    routes: () => get<RoutesSetting>('/api/v1/station/routes'),
    setRoutes: (routes: Record<string, string>) =>
        put<{ routes: Record<string, string>; problems: string[] }>('/api/v1/station/routes', { routes }),
    mode: () => get<ModeView>('/api/v1/station/mode'),
    setMode: (mode: PanelMode) => post<ModeView>('/api/v1/station/mode', { mode }),
    /** «Остаться»: режим, в который шлюз перешёл сам, становится выбором человека. */
    keepMode: () => post<ModeView>('/api/v1/station/mode/keep'),
    availability: () => get<AvailabilityView>('/api/v1/availability'),
    placement: (station?: string) => get<PlacementView>(forStation('/api/v1/placement', station)),
    startMove: (body: {
        to: MoveTo;
        /** Колонка, чей шлюз переносим: без адреса берётся её адрес. */
        station?: string;
        address?: string;
        gateway?: string;
        settingsHome?: 'station' | 'pc';
        remove?: boolean;
    }) => post<MoveJob>('/api/v1/placement/move', body),
    moveJob: (id: string) => get<MoveJob>(`/api/v1/placement/move/${encodeURIComponent(id)}`),
    backupStation: (station?: string) =>
        post<{ backup: StationGatewayView['backups'][number] }>(forStation('/api/v1/placement/backup', station)),
    restoreStationSettings: (id: string, station?: string) =>
        post<{ restored: string }>(forStation('/api/v1/placement/restore', station), { id }),
    setSettingsHome: (home: 'station' | 'pc', station?: string) =>
        post<unknown>(forStation('/api/v1/placement/settings-home', station), { home }),
    setSafety: (on: boolean, station?: string) => post<unknown>(forStation('/api/v1/placement/safety', station), { on }),
    setStationAutoUpdate: (on: boolean, station?: string) =>
        post<unknown>(forStation('/api/v1/placement/auto-update', station), { on }),
    updateStationGateway: (station?: string) =>
        post<{ updating: boolean }>(forStation('/api/v1/placement/update-station', station)),
    inspectStation: (station?: string) =>
        post<{
            installed: boolean;
            running: boolean;
            disabled: boolean;
            local: boolean;
            fired?: string | null;
            status?: Record<string, unknown> | null;
            log?: string;
        }>(forStation('/api/v1/placement/inspect', station)),
    retryStation: (station?: string) => post<unknown>(forStation('/api/v1/placement/retry', station)),
    stationVoice: (to: 'local' | 'yandex', station?: string) =>
        post<unknown>(forStation('/api/v1/placement/station-voice', station), { to }),
    /** Шлюз на колонке поставлен вручную: компьютер связывается с ним по адресу и ключу пары. */
    linkStation: (address: string, port: number, key: string) => post<StationGatewayView>('/api/v1/placement/link', { address, port, key }),
    /** Шлюз на колонке: голос колонки — Яндексу или обратно себе, без компьютера. */
    giveBack: () => post<unknown>('/api/v1/placement/give-back'),
    pointLocal: () => post<unknown>('/api/v1/placement/point-local'),
    homeServer: () => get<HomeServerStatus>('/api/v1/home-server'),
    setHomeServer: (url: string, key?: string) => put<HomeServerStatus>('/api/v1/home-server', { url, key }),
    removeHomeServer: () => del<HomeServerStatus>('/api/v1/home-server'),
    checkHomeServer: () => post<HomeServerStatus>('/api/v1/home-server/check'),
    /** Пульт колонки: адрес ПК, ключ, «ПК помогает колонке» — что не передано, остаётся. */
    linkHomeServer: (body: { url?: string; key?: string; enabled?: boolean }) => post<HomeServerStatus>('/api/v1/home-server/link', body),
    /** Пульт ПК: «ПК помогает колонке» — колонка опирается на этот ПК или работает одна. */
    setStationHelper: (on: boolean, station?: string) =>
        post<StationGatewayView>(forStation('/api/v1/placement/helper', station), { on }),
    say: (text: string) => post<{ outcome: string }>('/api/v1/station/sessions/latest/say', { text }),
    stations: () => get<Station[]>('/api/v1/stations'),
    // Журнал запросов: что спрашивали и что ответили, по странице за раз; сколько дней его хранить.
    requests: (query: { limit?: number; before?: string; device?: string } = {}) => {
        const params = new URLSearchParams();
        if (query.limit) params.set('limit', String(query.limit));
        if (query.before) params.set('before', query.before);
        if (query.device) params.set('device', query.device);
        const search = params.toString();
        return get<{ requests: RequestView[]; keepDays: number }>(`/api/v1/requests${search ? `?${search}` : ''}`);
    },
    keepRequests: (days: number) => put<{ days: number }>('/api/v1/requests/keep', { days }),
    updateStation: (device: string, change: { status?: Station['status']; name?: string; model?: string | null; color?: string | null; sticker?: boolean | null }) =>
        patch<Station>(`/api/v1/stations/${encodeURIComponent(device)}`, change),
    // Подключение колонки из пульта (gateway/src/stations/connect)
    /** gateway — адреса этого шлюза в домашней сети, как их наберёт колонка (пусто в Docker). */
    /** port — на какой порт колонка пойдёт, если адрес шлюза введён без порта: порт самого шлюза. */
    discoverStations: () => get<{ stations: FoundStation[]; gateway?: string[]; port?: number }>('/api/v1/stations/connect/discover'),
    /** Подключённые колонки, которые всё ещё ходят на прежний порт этого компьютера: шлюз с тех пор сменил порт. */
    movedGateway: () => get<MovedGateway[]>('/api/v1/stations/connect/moved'),
    checkStation: (address: string, gateway?: string) => post<StationCheck>('/api/v1/stations/connect/check', { address, gateway }),
    connectStation: (address: string, gateway: string) => post<ConnectJob>('/api/v1/stations/connect', { address, gateway }),
    /** previous — «как было» перед последним подключением; yandex — без шлюза, как до любого. */
    restoreStation: (address: string, to: 'previous' | 'yandex' = 'yandex') => post<ConnectJob>('/api/v1/stations/connect/restore', { address, to }),
    connectJob: (id: string) => get<ConnectJob>(`/api/v1/stations/connect/jobs/${encodeURIComponent(id)}`),
    // Вход
    panelState: () => get<{ configured: boolean; accounts: number; needsCode: boolean }>('/api/v1/auth/state'),
    setup: (password: string, code?: string) => post<{ token: string; user: PanelUser }>('/api/v1/auth/setup', { password, code }),
    login: (name: string | undefined, password: string) =>
        post<{ token: string; user: PanelUser }>('/api/v1/auth/login', { name, password }),
    me: () => get<Me>('/api/v1/auth/me'),
    changePassword: (current: string, next: string) => put<{ token: string }>('/api/v1/auth/password', { current, next }),
    logoutEverywhere: () => post<void>('/api/v1/auth/logout-everywhere'),
    // Люди и права
    catalog: () => get<AccessCatalog>('/api/v1/access/catalog'),
    people: () => get<Person[]>('/api/v1/users'),
    addPerson: (person: { name: string; password?: string; role: Role }) => post<Person>('/api/v1/users', person),
    updatePerson: (
        id: string,
        change: Partial<{ name: string; role: Role; status: Person['status']; password: string; restrictions: Person['restrictions'] }>,
    ) => patch<Person>(`/api/v1/users/${id}`, change),
    removePerson: (id: string) => del<void>(`/api/v1/users/${id}`),
    household: () => get<{ role: Role; restrictions: Person['restrictions'] }>('/api/v1/household'),
    setHousehold: (value: { role: Role }) => put<{ role: Role; restrictions: Person['restrictions'] }>('/api/v1/household', value),
    apiKeys: () => get<ApiKey[]>('/api/v1/api-keys'),
    createApiKey: (name: string, grants: string[]) => post<{ key: ApiKey; secret: string }>('/api/v1/api-keys', { name, grants }),
    revokeApiKey: (id: string) => del<void>(`/api/v1/api-keys/${id}`),
    audit: (limit = 50) => get<AuditEntry[]>(`/api/v1/audit?limit=${limit}`),
    connectors: () => get<{ connectors: ConnectorView[]; kinds: Array<{ id: string; title: string }> }>('/api/v1/connectors'),
    // Подключения, которые настраиваются здесь: применяются сразу, без перезапуска шлюза.
    connectorSettings: (id: string) => get<ConnectorSettings>(`/api/v1/connectors/${encodeURIComponent(id)}/settings`),
    updateConnector: (id: string, change: Record<string, unknown>) =>
        put<ConnectorSettings>(`/api/v1/connectors/${encodeURIComponent(id)}/settings`, change),
    clearConnector: (id: string) => del<ConnectorSettings>(`/api/v1/connectors/${encodeURIComponent(id)}/settings`),
    checkConnector: (id: string) => post<ConnectorSettings>(`/api/v1/connectors/${encodeURIComponent(id)}/check`),
    telegramChats: () => get<TelegramStranger[]>('/api/v1/connectors/telegram/chats'),
    newsCatalog: () => get<NewsCatalog>('/api/v1/connectors/news/catalog'),
    previewFeed: (url: string) => post<FeedPreview>('/api/v1/connectors/news/preview', { url }),
    updates: () => get<UpdateStatus>('/api/v1/updates'),
    checkUpdates: () => post<UpdateStatus>('/api/v1/updates/check'),
    installUpdate: () => post<UpdateStatus>('/api/v1/updates/install'),
    // Магазин моделей: каталог с реальностью, скачивание, активация, рестарты серверов.
    models: () => get<ModelsView>('/api/v1/models'),
    installVoice: (id: string) => post<DownloadState>(`/api/v1/models/tts/${encodeURIComponent(id)}/install`),
    cancelVoice: (id: string) => post<{ canceled: boolean }>(`/api/v1/models/tts/${encodeURIComponent(id)}/cancel`),
    uninstallVoice: (id: string) => del<{ deleted: boolean }>(`/api/v1/models/tts/${encodeURIComponent(id)}`),
    placeSileroFile: (file: File) =>
        fetch(`/api/v1/models/tts/silero/file?name=${encodeURIComponent(file.name)}`, {
            method: 'POST',
            headers: { ...authHeaders(), 'Content-Type': 'application/octet-stream' },
            body: file,
        }).then(async (response) => {
            if (!response.ok) throw new ApiError(response.status, await response.text().catch(() => ''));
            return (await response.json()) as { message: { placed: boolean } | null };
        }),
    /**
     * Выбрать голос. Без модели или файла — 409 с needs в errors (needsOf); с install — шлюз ставит их и включит голос
     * сам (202, pending). Памяти впритык — 428; тот же запрос с anyway включает всё равно.
     */
    activateTts: (modelId: string, voiceId?: string, anyway = false, install = false) =>
        post<{ needsRestart?: boolean; pending?: TtsPending | null }>('/api/v1/models/tts/activate', {
            modelId,
            voiceId,
            ...(anyway ? { anyway: true } : {}),
            ...(install ? { install: true } : {}),
        }),
    /** Забыть отложенный выбор голоса: модель доставится, но голос сам не включится. */
    cancelTtsPending: () => del<unknown>('/api/v1/models/tts/pending'),
    activateAsr: (id: string) => post<{ needsRestart: boolean }>('/api/v1/models/asr/activate', { id }),
    restartServer: (which: 'asr' | 'tts') => post<{ restarted: boolean }>(`/api/v1/models/servers/${which}/restart`),
    // Свой голос: модели, дубли, отпечаток. Выбор голоса — тот же activateTts с id `own:<голос>`; при памяти впритык
    // шлюз отвечает 428 с предупреждением, и тот же запрос с anyway запускает всё равно.
    voiceLab: () => get<VoiceLabView>('/api/v1/voice-lab'),
    voiceLabRecheck: () => post<VoiceLabView>('/api/v1/voice-lab/platform/recheck'),
    installEngine: (id: CloneEngineId) => post<VoiceLabJob>(`/api/v1/voice-lab/engines/${id}/install`),
    addTake: (wav: Blob, passage?: number) =>
        fetch(`/api/v1/voice-lab/draft/takes${passage ? `?passage=${passage}` : ''}`, {
            method: 'POST',
            headers: { ...authHeaders(), 'Content-Type': 'audio/wav' },
            body: wav,
        }).then(async (response) => {
            if (!response.ok) throw new ApiError(response.status, await response.text().catch(() => ''));
            return ((await response.json()) as { message: VoiceLabTake }).message;
        }),
    removeTake: (n: number) => del<{ deleted: boolean }>(`/api/v1/voice-lab/draft/takes/${n}`),
    clearDraft: () => del<{ cleared: boolean }>('/api/v1/voice-lab/draft'),
    imprint: (engine: CloneEngineId, name: string, anyway = false) =>
        post<VoiceLabJob>('/api/v1/voice-lab/imprint', { engine, name, ...(anyway ? { anyway: true } : {}) }),
    stopVoiceJob: () => post<{ stopped: boolean }>('/api/v1/voice-lab/job/stop'),
    renameVoice: (id: string, name: string) => patch<OwnVoice>(`/api/v1/voice-lab/voices/${encodeURIComponent(id)}`, { name }),
    deleteVoice: (id: string) => del<{ deleted: boolean }>(`/api/v1/voice-lab/voices/${encodeURIComponent(id)}`),
    takeAudio: (n: number) => fetchAudio(`/api/v1/voice-lab/draft/takes/${n}`),
    sampleAudio: (id: string, n: number) => fetchAudio(`/api/v1/voice-lab/voices/${encodeURIComponent(id)}/samples/${n}`),
    /** Свой голос заново под другую модель — из записей, что хранятся с голосом. */
    reimprintVoice: (id: string, engine: string) =>
        post<VoiceLabJob>(`/api/v1/voice-lab/voices/${encodeURIComponent(id)}/reimprint`, { engine }),
    // Характер Алисы: применяется со следующего вопроса, без рестарта.
    brain: () => get<BrainView>('/api/v1/brain'),
    addProvider: (input: ProviderInput) => post<ProviderView>('/api/v1/brain/providers', input),
    updateProvider: (id: string, input: ProviderInput) => patch<ProviderView>(`/api/v1/brain/providers/${encodeURIComponent(id)}`, input),
    removeProvider: (id: string) => del<void>(`/api/v1/brain/providers/${encodeURIComponent(id)}`),
    checkProvider: (id: string, model?: string) => post<ProviderCheck>(`/api/v1/brain/providers/${encodeURIComponent(id)}/check`, model ? { model } : {}),
    providerModels: (id: string) => post<{ models: string[] }>(`/api/v1/brain/providers/${encodeURIComponent(id)}/models`),
    chooseBrain: (choice: { fast?: BrainPick | null; smart?: BrainPick | null; effort?: LlmEffort }) => put<BrainView>('/api/v1/brain/choice', choice),

    mcp: () => get<McpView>('/api/v1/mcp'),
    addMcpServer: (input: McpServerInput) => post<McpServer>('/api/v1/mcp/servers', input),
    importMcp: (json: string) => post<{ added: McpServer[]; problems: string[] }>('/api/v1/mcp/import', { json }),
    updateMcpServer: (id: string, input: McpServerInput) => patch<McpServer>(`/api/v1/mcp/servers/${encodeURIComponent(id)}`, input),
    removeMcpServer: (id: string) => del<void>(`/api/v1/mcp/servers/${encodeURIComponent(id)}`),
    setMcpTool: (id: string, tool: string, change: { on?: boolean; hint?: string }) =>
        put<McpServer>(`/api/v1/mcp/servers/${encodeURIComponent(id)}/tools/${encodeURIComponent(tool)}`, change),
    approveMcpInstructions: (id: string) => post<McpServer>(`/api/v1/mcp/servers/${encodeURIComponent(id)}/instructions`),
    reconnectMcp: (id: string) => post<McpServer>(`/api/v1/mcp/servers/${encodeURIComponent(id)}/reconnect`),
    loginMcp: (id: string) => post<{ url?: string; done?: boolean }>(`/api/v1/mcp/servers/${encodeURIComponent(id)}/login`),
    completeMcpLogin: (url: string) => post<{ id: string; title: string }>('/api/v1/mcp/oauth/complete', { url }),
    tryMcpTool: (id: string, tool: string, args: Record<string, unknown>) =>
        post<{ content: string; isError: boolean; ms: number }>(`/api/v1/mcp/servers/${encodeURIComponent(id)}/tools/${encodeURIComponent(tool)}/try`, { arguments: args }),

    persona: () => get<PersonaInfo>('/api/v1/persona'),
    setPreset: (presetId: string) => put<PersonaInfo>('/api/v1/persona', { presetId }),
    setPersonaText: (text: string) => put<PersonaInfo>('/api/v1/persona', { text }),
    /** Правка текста выбранного персонажа: он остаётся выбранным, с пометкой «правили вручную». */
    setCharacterText: (presetId: string, text: string) => put<PersonaInfo>('/api/v1/persona', { presetId, text }),

    // Персонажи: встроенные и из пакетов, с состоянием голоса. Выбор — одно действие: характер и голос.
    characters: () => get<CharactersView>('/api/v1/characters'),
    chooseCharacter: (id: string, input: ChooseInput = {}) =>
        post<ChooseResult>(`/api/v1/characters/${encodeURIComponent(id)}/choose`, input),

    // Пакеты любого вида и общий каталог. Ставит и удаляет только человек с правом на этот вид.
    packages: () => get<PackagesView>('/api/v1/packages'),
    packagesResources: () => get<PackagesResources>('/api/v1/packages/resources'),
    packageThemes: () => get<{ themes: Array<{ package: string; version: string; origin: PackageOrigin; theme: unknown }> }>('/api/v1/packages/themes'),
    packagePanelParts: () => get<{ parts: PanelPart[] }>('/api/v1/packages/panel'),
    /** Пульт плагина зовёт свой бэкенд: только функции из contributes.panel.calls. */
    callPackage: <T = unknown>(id: string, fn: string, input: unknown = {}) =>
        post<{ result: T }>(`/api/v1/packages/${encodeURIComponent(id)}/call/${encodeURIComponent(fn)}`, input).then((answer) => answer.result),
    /** «Сделать свою копию»: id копии — my-<id>, если не задан. */
    forkPackage: (id: string, newId?: string) => post<PackageView>(`/api/v1/packages/${encodeURIComponent(id)}/fork`, newId ? { id: newId } : {}),
    reloadPackage: (id: string) => post<PackageView>(`/api/v1/packages/${encodeURIComponent(id)}/reload`),
    /** Назад к прошлой версии: новейшая уходит. */
    rollbackPackage: (id: string) => post<PackageView>(`/api/v1/packages/${encodeURIComponent(id)}/rollback`),
    workshop: () => get<WorkshopView>('/api/v1/workshop'),
    createDraft: (body: { id: string; kind?: 'plugin' | 'theme'; title?: string; openapi?: string }) => post<DraftView>('/api/v1/workshop/drafts', body),
    /** «Открыть в мастерской»: черновик, а нет его — из своего поставленного пакета. */
    openDraft: (id: string) => post<DraftView>(`/api/v1/workshop/drafts/${encodeURIComponent(id)}/open`),
    deleteDraft: (id: string) => del<unknown>(`/api/v1/workshop/drafts/${encodeURIComponent(id)}`),
    readDraftFile: (id: string, path: string) =>
        get<{ path: string; content: string }>(`/api/v1/workshop/drafts/${encodeURIComponent(id)}/files/${path.split('/').map(encodeURIComponent).join('/')}`),
    writeDraftFile: (id: string, path: string, content: string) =>
        put<DraftView>(`/api/v1/workshop/drafts/${encodeURIComponent(id)}/files/${path.split('/').map(encodeURIComponent).join('/')}`, { content }),
    deleteDraftFile: (id: string, path: string) =>
        del<DraftView>(`/api/v1/workshop/drafts/${encodeURIComponent(id)}/files/${path.split('/').map(encodeURIComponent).join('/')}`),
    checkDraft: (id: string) => post<CheckReport>(`/api/v1/workshop/drafts/${encodeURIComponent(id)}/check`),
    runDraft: (id: string, fn: string, input: unknown) => post<RunReport>(`/api/v1/workshop/drafts/${encodeURIComponent(id)}/run`, { function: fn, input }),
    tryDraftPhrase: (id: string, text: string) => post<PhraseReport>(`/api/v1/workshop/drafts/${encodeURIComponent(id)}/phrase`, { text }),
    draftJournal: (id: string) => get<{ lines: JournalLine[] }>(`/api/v1/workshop/drafts/${encodeURIComponent(id)}/journal`),
    installDraft: (id: string, approve: boolean) => post<InstallReport>(`/api/v1/workshop/drafts/${encodeURIComponent(id)}/install`, { approve }),
    installPlan: (id: string) => get<InstallPlan>(`/api/v1/workshop/drafts/${encodeURIComponent(id)}/install`),
    startTrial: (id: string, approve: boolean, minutes = 10) => post<InstallReport>(`/api/v1/workshop/drafts/${encodeURIComponent(id)}/trial`, { approve, minutes }),
    endTrial: (id: string, keep: boolean) => del<{ message: string; draft?: DraftView | null }>(`/api/v1/workshop/drafts/${encodeURIComponent(id)}/trial${keep ? '?keep=true' : ''}`),
    workshopAgent: () => get<WorkshopAgentModel>('/api/v1/workshop/agent'),
    setWorkshopAgent: (choice: BrainPick | null) => put<WorkshopAgentModel>('/api/v1/workshop/agent', { choice }),
    setPackageSettings: (id: string, values: Record<string, unknown>) => put<PackageView>(`/api/v1/packages/${encodeURIComponent(id)}/settings`, values),
    enablePackage: (id: string) => post<PackageView>(`/api/v1/packages/${encodeURIComponent(id)}/enable`),
    disablePackage: (id: string) => post<PackageView>(`/api/v1/packages/${encodeURIComponent(id)}/disable`),
    packagesCatalog: (refresh = false) => get<CatalogView>(`/api/v1/packages/catalog${refresh ? '?refresh=1' : ''}`),
    /** Выключатель «Не загружать каталог» (настройка packages_catalog). */
    setPackagesCatalog: (enabled: boolean) => put<CatalogView>('/api/v1/packages/catalog', { enabled }),
    installPackage: (id: string, version?: string) =>
        post<PackageView>(`/api/v1/packages/${encodeURIComponent(id)}/install`, version ? { version } : {}),
    updatePackage: (id: string) => post<PackageView>(`/api/v1/packages/${encodeURIComponent(id)}/update`),
    removePackage: (id: string) => del<unknown>(`/api/v1/packages/${encodeURIComponent(id)}`),
    /**
     * Файл шлюза с ключом входа (картинка, фраза-пробник): адреса /api/… и /media/… — как их прислал шлюз. Чужой адрес
     * не запрашивается: ключ входа уходит только своему шлюзу.
     */
    media: (url: string) =>
        url.startsWith('/') && !url.startsWith('//') ? fetchAudio(url) : Promise.reject(new ApiError(400, 'Файл не со шлюза')),
};

/** Что сказать пользователю об ошибке запроса: текст шлюза, если он есть. */
export function problemOf(error: unknown): string {
    if (!(error instanceof ApiError)) return 'Шлюз недоступен — попробуйте ещё раз.';
    try {
        return String(JSON.parse(error.body)?.message ?? error.body).slice(0, 200);
    } catch {
        return error.body.slice(0, 200) || `Ошибка ${error.status}`;
    }
}

/** Поле errors ответа-ошибки шлюза: подробности к сообщению (цифры памяти у 428, needs у 409). */
export function errorsOf(error: unknown): Record<string, unknown> | null {
    if (!(error instanceof ApiError)) return null;
    try {
        const errors = JSON.parse(error.body)?.errors;
        return errors && typeof errors === 'object' ? (errors as Record<string, unknown>) : null;
    } catch {
        return null;
    }
}

/** Голосу не хватает модели или файла (409): что именно — для окна «нужна модель». */
export function needsOf(error: unknown): VoiceNeeds | null {
    if (!(error instanceof ApiError) || error.status !== 409) return null;
    const needs = errorsOf(error)?.needs;
    return needs && typeof needs === 'object' && typeof (needs as VoiceNeeds).id === 'string' ? (needs as VoiceNeeds) : null;
}

/** Шлюз предупредил: памяти впритык (428) — можно запустить «всё равно». */
export function isTight(error: unknown): boolean {
    return error instanceof ApiError && error.status === 428;
}

/** «1 возможность, 2 возможности, 5 возможностей». */
export function plural(count: number, one: string, few: string, many: string): string {
    const tens = count % 100;
    const units = count % 10;
    if (tens >= 11 && tens <= 14) return many;
    if (units === 1) return one;
    if (units >= 2 && units <= 4) return few;
    return many;
}
