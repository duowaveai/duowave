// Виджеты колонки на столе (docs/plans/desk.md, этап 2): её таймеры — с именами, что сказали голосом («поставь
// таймер на чай на пять минут»), — и что она играет: обложка, мягкий тон фоном, волна дорожки вместо полосы, назад /
// пауза / дальше; чёрная кнопка игры — лайм остаётся только у «Сказать». Данные — /api/v1/station/timers и
// /api/v1/station/player; перечитываются, когда шлюз скажет `station.state`, и когда колонка ответила на вопрос. У
// помощника колонки может не быть на связи — тогда компьютер отдаёт копию, и виджеты показывают её копией: с её
// временем, без отсчёта и без кнопок. Живое радио паузы не знает: вместо ⏸ у него ⏹ «Остановить», а ▶ включает его
// снова. Чем управляет Яндекс, шлюз кнопкам не отдаёт — виджет говорит, что сказать колонке.

import { useEffect, useMemo, useState } from 'preact/hooks';

import { ApiError, NowPlaying, PlayerCommand, RequestView, StationAnswer, TimerView, api, can, problemOf, stationCall, stationRead } from '../api';
import { IconHeart, IconMusic, IconNext, IconPause, IconPlay, IconPrev, IconSpeaker } from '../icons';
import { usePoll } from '../poll';
import { usePanel } from '../shell/context';
import { useMediaUrl } from '../ui/media';

/**
 * Перечитать: раз в `every` мс, с каждым законченным вопросом колонки и по переменам на шлюзе — не чаще раза в
 * несколько секунд, без наложений: пока идёт чтение, нового нет, а уход со стола его обрывает (poll.ts). У помощника —
 * у шлюза колонки, через проход (`station` — её ключ), у шлюза — у себя. Третье — когда снята копия, если колонки
 * нет на связи и ответил компьютер тем, что помнит (null — ответ живой).
 */
function useStationRead<T>(
    read: (device: string | undefined, station: string | undefined, signal: AbortSignal) => Promise<StationAnswer<T>>,
    every: number,
): [T | null, () => void, number | null] {
    const panel = usePanel();
    const device = panel.current?.device;
    const station = panel.role.role === 'helper' ? panel.role.station?.key : undefined;
    const [value, setValue] = useState<StationAnswer<T> | null>(null);
    // Законченный вопрос колонки: таймер могли поставить, музыку — включить.
    const last = panel.feed.requests.find((request) => request.status !== 'active' && (!device || request.device === device))?.id;
    const reread = usePoll({
        deps: [device, station, last],
        every,
        version: panel.version,
        read: (signal) => read(device, station, signal).then((fresh) => !signal.aborted && setValue(fresh)),
    });
    return [value?.value ?? null, reread, value?.copyAt ?? null];
}

/** Ответ своего шлюза — всегда живой. */
const fresh = <T,>(value: T): StationAnswer<T> => ({ value, copyAt: null });

/** Таймеры колонки: свои — у своего шлюза, у помощника — у шлюза колонки. */
const readTimers = (device: string | undefined, station: string | undefined, signal: AbortSignal): Promise<StationAnswer<{ timers: TimerView[] }>> =>
    station ? stationRead<{ timers: TimerView[] }>(station, '/api/v1/station/timers', signal) : api.timers(device, signal).then(fresh);

const readPlayer = (device: string | undefined, station: string | undefined, signal: AbortSignal): Promise<StationAnswer<NowPlaying>> =>
    station ? stationRead<NowPlaying>(station, '/api/v1/station/player', signal) : api.player(device, signal).then(fresh);

const playerCommand = (action: PlayerCommand, device?: string, station?: string) =>
    station
        ? stationCall<{ pushed: number }>(station, 'POST', `/api/v1/station/player/${action}`, {})
        : api.playerCommand(action, device);

/** Последние вопросы колонки у помощника: её журнал через проход (у шлюза — живой канал, desk-widgets.tsx). */
export const readStationRequests = (station: string, signal?: AbortSignal): Promise<RequestView[]> =>
    stationCall<{ requests: RequestView[] }>(station, 'GET', '/api/v1/requests?limit=4', undefined, signal).then((answer) => answer.requests);

/** Перерисовка раз в секунду, пока `on`: отсчёт таймеров и позиция дорожки идут без запросов (время — Date.now()). */
function useTick(on: boolean): void {
    const [, setNow] = useState(0);
    useEffect(() => {
        if (!on) return;
        const id = window.setInterval(() => setNow(Date.now()), 1000);
        return () => window.clearInterval(id);
    }, [on]);
}

/** «копия от 14:05» (не сегодня — с датой): колонки нет на связи, это то, что компьютер помнит. */
function copyLine(at: number): string {
    const date = new Date(at);
    const time = date.toLocaleTimeString('ru-RU', { hour: '2-digit', minute: '2-digit' });
    return `копия от ${date.toDateString() === new Date().toDateString() ? time : `${date.toLocaleDateString('ru-RU')} ${time}`}`;
}

/** «03:12», «1:05:00». */
export function clockOf(seconds: number): string {
    const s = Math.max(0, Math.round(seconds));
    const h = Math.floor(s / 3600);
    const m = Math.floor((s % 3600) / 60);
    const rest = s % 60;
    const two = (n: number) => String(n).padStart(2, '0');
    return h > 0 ? `${h}:${two(m)}:${two(rest)}` : `${two(m)}:${two(rest)}`;
}

/**
 * «Таймеры»: имя, сколько осталось крупно, полоса прошедшего. Свои и те, что поставил Яндекс. Отсчёт — от того, когда
 * таймер прозвенит (`endsAt`, по часам шлюза): перечитывание его не сбивает. Копия — как было на её время, без отсчёта.
 */
export function TimersWidget() {
    const panel = usePanel();
    const [view, , copyAt] = useStationRead(readTimers, 15_000);
    const timers = view?.timers ?? [];
    useTick(timers.length > 0 && copyAt === null);
    if (!view) return <p class="w-cap m-0">Читаю…</p>;
    if (timers.length === 0)
        return (
            <p class="w-cap m-0">
                {copyAt !== null ? `Таймеров не было (${copyLine(copyAt)}).` : 'Таймеров нет. Скажите колонке: «поставь таймер на чай на пять минут».'}
            </p>
        );
    // Часы шлюза: у копии — время копии, у живого ответа — сейчас, с поправкой на часы шлюза.
    const at = copyAt ?? Date.now() + panel.feed.skew;
    return (
        <div class="flex flex-col gap-12">
            {copyAt !== null && <p class="w-cap m-0">{copyLine(copyAt)} — колонки нет на связи</p>}
            {timers.slice(0, 3).map((timer: TimerView) => {
                const left = timer.paused || !Number.isFinite(timer.endsAt) ? timer.left : Math.max(0, (timer.endsAt - at) / 1000);
                const done = timer.seconds > 0 ? Math.min(100, Math.max(0, ((timer.seconds - left) / timer.seconds) * 100)) : 0;
                return (
                    <div key={timer.id}>
                        <div class="flex items-baseline justify-between gap-8">
                            <span class="w-text">
                                {timer.name ? timer.name[0].toUpperCase() + timer.name.slice(1) : 'Таймер'}
                                {timer.paused && <span class="w-cap"> · на паузе</span>}
                            </span>
                            <span class="w-num">{clockOf(left)}</span>
                        </div>
                        <div class="w-bar mt-6" role="img" aria-label={`Прошло ${Math.round(done)} процентов`}>
                            <span style={{ width: `${done}%` }} />
                        </div>
                    </div>
                );
            })}
        </div>
    );
}

/** Мягкий тон под обложку: от названия, чтобы у дорожки был свой и он не прыгал. */
const TONES = ['#ECF5F3', '#F3EEF8', '#F6F1E7', '#EEF2F8', '#F5EEEE', '#EEF5EA'];
const GRADIENTS = [
    'linear-gradient(140deg,#3FE0C5 0%,#7C6CF0 100%)',
    'linear-gradient(140deg,#FFB547 0%,#F0617C 100%)',
    'linear-gradient(140deg,#84CC16 0%,#0EA5E9 100%)',
    'linear-gradient(140deg,#A78BFA 0%,#F472B6 100%)',
];
function hashOf(text: string): number {
    let hash = 7;
    for (const char of text) hash = (hash * 31 + char.charCodeAt(0)) >>> 0;
    return hash;
}

/** Волна дорожки: высоты полосок из названия — одна и та же у одной дорожки. */
function barsOf(title: string, count: number): number[] {
    let seed = hashOf(title) || 1;
    const bars: number[] = [];
    for (let i = 0; i < count; i += 1) {
        seed = (seed * 1103515245 + 12345) >>> 0;
        const wave = Math.sin((i / count) * Math.PI * 3) * 0.35 + 0.6;
        bars.push(Math.round(5 + ((seed % 1000) / 1000) * 20 * wave));
    }
    return bars;
}

function minutesSince(since: number, now: number): string {
    const minutes = Math.max(0, Math.round((now - since) / 60_000));
    if (minutes < 1) return 'только что';
    if (minutes < 60) return `${minutes} мин`;
    return `${Math.floor(minutes / 60)} ч ${minutes % 60} мин`;
}

/**
 * Обложка, которую пульт может показать: адрес своего шлюза (у помощника — шлюза колонки, через проход) — с ключом
 * входа, в blob; data: и blob: — как есть. Чужой адрес (значок станции радио по https) политика пульта не пустит
 * (`img-src 'self'`): тогда обложка — градиент.
 */
function useCover(url: string | undefined, station?: string): string | null {
    let inline: string | null = null;
    let own: string | null = null;
    if (url && /^(data|blob):/i.test(url)) inline = url;
    else if (url) {
        try {
            const parsed = new URL(url, location.href);
            if (parsed.origin === location.origin) own = `${parsed.pathname}${parsed.search}`;
        } catch {
            // не адрес — без обложки
        }
    }
    const fetched = useMediaUrl(own && station ? `/station/${encodeURIComponent(station)}${own}` : own);
    return inline ?? fetched;
}

/** ⏹ — у живого радио вместо паузы (рисунок в духе значков пульта: залитый, со скруглением). */
function StopGlyph({ size }: { size: number }) {
    return (
        <svg aria-hidden="true" width={size} height={size} viewBox="0 0 24 24" fill="currentColor">
            <rect x="6" y="6" width="12" height="12" rx="1.6" />
        </svg>
    );
}

/** Сколько видна подсказка «это играет Яндекс…» после нажатия. */
const NOTE_MS = 8000;

/** «Музыка»: что играет на колонке — обложка, название, волна дорожки и кнопки; радио — сколько в эфире. */
export function MusicWidget() {
    const panel = usePanel();
    const station = panel.role.role === 'helper' ? panel.role.station?.key : undefined;
    const [now, reread, copyAt] = useStationRead(readPlayer, 20_000);
    const [busy, setBusy] = useState<PlayerCommand | null>(null);
    const [error, setError] = useState<string | null>(null);
    /** Отказ шлюза (409): кнопкой этого не сделать — например, играет Яндекс; не ошибка, а что сказать колонке. */
    const [note, setNote] = useState<string | null>(null);
    const [broken, setBroken] = useState<string | null>(null);
    useEffect(() => {
        if (!note) return;
        const timer = window.setTimeout(() => setNote(null), NOTE_MS);
        return () => window.clearTimeout(timer);
    }, [note]);
    const cover = useCover(now?.imageUrl, station);
    const read = useMemo(() => Date.now(), [now]);
    const playing = now?.state === 'playing';
    // Копия не идёт: что играло на её время.
    const live = playing && copyAt === null;
    useTick(live);
    const title = now?.title;
    if (!now || !title || now.source === 'none')
        return (
            <section class="widget music" aria-label="Музыка">
                <div class="flex items-center gap-12">
                    <span class="music-cover music-cover-empty" aria-hidden="true">
                        <IconMusic size={22} />
                    </span>
                    <p class="w-cap m-0">
                        {!now
                            ? 'Читаю…'
                            : copyAt !== null
                              ? `Ничего не играло (${copyLine(copyAt)}).`
                              : 'Сейчас ничего не играет. Скажите: «включи радио» или «включи музыку».'}
                    </p>
                </div>
            </section>
        );
    const hash = hashOf(title);
    const radio = now.kind === 'FmRadio' || !now.durationMs;
    // Живое радио (наше или Яндекса): на паузу его не поставить — только остановить; ▶ потом включает снова.
    const onAir = now.kind === 'FmRadio';
    const position = (now.positionMs ?? 0) + (live ? Date.now() - read : 0);
    const length = now.durationMs ?? 0;
    const bars = barsOf(title, 52);
    const played = length > 0 ? Math.round((Math.min(position, length) / length) * bars.length) : 0;
    // Колонки нет на связи — управлять нечем: кнопки молчат до её возвращения.
    const mayControl = can('station.say');
    const away = copyAt !== null;
    const command = (action: PlayerCommand) => {
        setBusy(action);
        setError(null);
        setNote(null);
        void playerCommand(action, panel.current?.device, station)
            .then(() => window.setTimeout(reread, 1500))
            // 409 — шлюз не берёт это кнопкой («Это играет Яндекс… скажите колонке «Алиса, дальше»»): подсказка, не беда.
            .catch((e) => (e instanceof ApiError && e.status === 409 ? setNote(problemOf(e)) : setError(problemOf(e))))
            .finally(() => setBusy(null));
    };
    const play = playing ? (onAir ? 'stop' : 'pause') : 'resume';
    const playLabel = playing ? (onAir ? 'Остановить' : 'Пауза') : 'Играть';
    return (
        <section class="widget music" aria-label={`Музыка: ${playing ? 'играет' : 'стоит'} «${title}»`} style={{ '--music-tone': TONES[hash % TONES.length] }}>
            <div class="flex gap-12 items-center">
                <span class="music-cover" aria-hidden="true" style={{ background: GRADIENTS[hash % GRADIENTS.length] }}>
                    {cover && broken !== cover ? <img src={cover} alt="" onError={() => setBroken(cover)} /> : <span class="music-cover-sun" />}
                </span>
                <div class="flex-1 min-w-0">
                    <p class="music-where">
                        <IconSpeaker size={12} />
                        {away ? copyLine(copyAt) : playing ? 'играет на колонке' : now.state === 'paused' ? 'на паузе' : 'остановлено'}
                    </p>
                    <p class="music-title">{title}</p>
                    {now.subtitle && <p class="w-cap m-0 w-text">{now.subtitle}</p>}
                </div>
                {now.source === 'own' && !radio && mayControl && (
                    <button class="music-ctl self-start" aria-label="Нравится" disabled={busy !== null || away} onClick={() => command('like')}>
                        <IconHeart size={18} />
                    </button>
                )}
            </div>
            {radio ? (
                <p class="w-cap m-0">
                    {now.since ? `в эфире ${minutesSince(now.since, copyAt ?? Date.now())}` : now.source === 'station' ? 'играет Яндекс' : 'в эфире'}
                </p>
            ) : (
                <div>
                    <div class="music-wave" role="img" aria-label={`Прослушано ${clockOf(position / 1000)} из ${clockOf(length / 1000)}`}>
                        {bars.map((height, index) => (
                            <span key={index} class={index < played ? 'on' : ''} style={{ height: `${height}px` }} />
                        ))}
                    </div>
                    <div class="flex justify-between w-cap mt-2">
                        <span>{clockOf(position / 1000)}</span>
                        <span>−{clockOf(Math.max(0, length - position) / 1000)}</span>
                    </div>
                </div>
            )}
            {mayControl && (
                <div class="music-controls">
                    <button class="music-ctl" aria-label="Назад" disabled={busy !== null || away} onClick={() => command('previous')}>
                        <IconPrev size={18} />
                    </button>
                    <button class="music-play" aria-label={playLabel} title={playLabel} disabled={busy !== null || away} onClick={() => command(play)}>
                        {!playing ? <IconPlay size={16} /> : onAir ? <StopGlyph size={16} /> : <IconPause size={16} />}
                    </button>
                    <button class="music-ctl" aria-label="Дальше" disabled={busy !== null || away} onClick={() => command('next')}>
                        <IconNext size={18} />
                    </button>
                </div>
            )}
            {note && (
                <p class="w-cap m-0" role="status">
                    {note}
                </p>
            )}
            {error && (
                <p class="w-cap c-dot-offline m-0" role="alert">
                    {error}
                </p>
            )}
        </section>
    );
}
