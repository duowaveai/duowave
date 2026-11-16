// YouTube — приложение на Node для компьютера (docs/plans/speaker-alone.md, этап 3; docs/plugins/apps/youtube.md).
// «Включи с ютуба Кино, группа крови»: поиск и микс «похожее» — открытым API сайта (lib/innertube.mjs), звук — yt-dlp
// и ffmpeg в MP3 (lib/audio.mjs), колонке — `/stream/<id>.mp3` через шлюз. Очередь — плеер шлюза: «дальше», «пауза»,
// «продолжи» — встроенные команды, «что играет» шлюз отвечает по названию трека, «добавь в очередь» — поле `add`.
import http from 'node:http';
import path from 'node:path';

import { Audio, VIDEO_ID } from './lib/audio.mjs';
import { YouTubeError, cover, mix, pick, search } from './lib/innertube.mjs';
import { kindOf, sayError, words } from './lib/words.mjs';
import { YtDlp } from './lib/ytdlp.mjs';

const { PORT, DUOWAVE_URL, DUOWAVE_APP_TOKEN } = process.env;
const DATA = process.env.DUOWAVE_APP_DATA ?? path.join(process.cwd(), 'data');
// Сколько похожего ставить в очередь за найденным: микс YouTube отдаёт около 25.
const MIX_LIMIT = 24;
// Одна и та же беда вслух — не чаще раза в 10 минут.
const SAY_AGAIN_MS = 10 * 60_000;
// Подряд не играет — дальше не перескакиваем: значит, беда не в одном видео.
const SKIPS_IN_A_ROW = 3;

function host(method, route, body) {
    return fetch(`${DUOWAVE_URL}${route}`, {
        method,
        headers: { authorization: `Bearer ${DUOWAVE_APP_TOKEN}`, 'content-type': 'application/json' },
        body: body === undefined ? undefined : JSON.stringify(body),
        signal: AbortSignal.timeout(3000),
    });
}

/** В журнал приложения в пульте; без шлюза (проверка руками) — в консоль. */
function log(line, level = 'info') {
    if (!DUOWAVE_URL) return console.log(line);
    host('POST', '/api/v1/apps/host/log', { level, message: line }).catch(() => console.log(line));
}

/** Язык дома сейчас: его меняют в «Настройках», поэтому спрашиваем при каждом ответе. */
async function locale() {
    try {
        const answer = await host('GET', '/api/v1/apps/host/locale');
        return (await answer.json())?.locale ?? 'ru';
    } catch {
        return 'ru';
    }
}

const ytdlp = new YtDlp(path.join(DATA, 'bin'), log);
const queues = new Map();
const audio = new Audio({
    dir: path.join(DATA, 'audio'),
    ytdlp,
    log,
    // Колонка взяла трек: следующий готовим сразу, чтобы «дальше» и переход играли без ожидания.
    onServe: (id) => {
        for (const queue of queues.values()) {
            const at = queue.ids.indexOf(id);
            if (at < 0) continue;
            queue.current = id;
            const next = queue.ids[at + 1];
            if (next) audio.prepare(next);
            return;
        }
    },
});

// «Вся колонка» без имени — для инструментов «Мозга»: им шлюз не говорит колонку, берём последнюю.
const ANY = '*';
let lastDevice = ANY;

/** Очередь колонки, как приложение её отдало плееру шлюза: для «следующий заранее» и «добавь в очередь». */
function queueOf(device) {
    const key = device ?? lastDevice;
    if (!queues.has(key)) queues.set(key, { ids: [], current: null, addedUntil: 0, playing: false, at: 0, skips: 0 });
    return queues.get(key);
}

/** Элемент для плеера шлюза: звук — свой поток приложения, остальное — для «Сейчас играет». */
function item(video) {
    return {
        id: video.id,
        title: video.title,
        subtitle: video.channel || undefined,
        imageUrl: cover(video.id),
        durationMs: video.durationMs ?? undefined,
        stream: `${video.id}.mp3`,
        format: 'MP3',
        type: 'Track',
    };
}

/** Слова человека без «с ютуба», «на YouTube»: остаётся то, что искать. */
export function cleanQuery(text) {
    return String(text ?? '')
        .replace(/(^|\s)(с|со|на|из|в|по)?\s*(ютуб[а-яё]*|youtube[а-яё]*|you tube)(?=\s|$)/giu, ' ')
        .replace(/\s+/g, ' ')
        .trim();
}

const spoken = new Map();
/** Сказать о беде вслух без запроса (фоновая ошибка): одна и та же — не чаще раза в 10 минут. */
async function sayLater(error, device) {
    const kind = kindOf(error);
    if (Date.now() - (spoken.get(kind) ?? 0) < SAY_AGAIN_MS) return;
    spoken.set(kind, Date.now());
    const text = sayError(error, await locale());
    log(`YouTube: ${error?.message ?? error} → «${text}»`, 'warn');
    await host('POST', '/api/v1/apps/host/say', { text, device: device && device !== ANY ? device : undefined }).catch(() => {});
}

/** Найти и собрать очередь: найденное первым, за ним микс. */
async function find(query, language, { withMix = true } = {}) {
    const found = await search(query, { language });
    const first = pick(found, query);
    if (!first) return [];
    if (!withMix) return [first];
    const similar = await mix(first.id, { language }).catch((error) => {
        log(`микс для ${first.id} не пришёл: ${error.message}`, 'warn');
        return [];
    });
    const seen = new Set([first.id]);
    const rest = similar.filter((video) => video.durationMs !== null && !seen.has(video.id) && seen.add(video.id));
    return [first, ...rest.slice(0, MIX_LIMIT)];
}

function remember(device, videos) {
    if (device) lastDevice = device;
    const queue = queueOf(device);
    queue.ids = videos.map((video) => video.id);
    queue.current = queue.ids[0] ?? null;
    queue.addedUntil = 0;
    queue.skips = 0;
}

/** «Включи с ютуба …»: найденное и похожее за ним. */
async function play(query, device) {
    const language = await locale();
    const said = words(language);
    const wanted = cleanQuery(query);
    if (!wanted) return { say: said.ask };
    let videos;
    try {
        videos = await find(wanted, language);
    } catch (error) {
        log(`поиск «${wanted}»: ${error.message}`, 'warn');
        return { say: sayError(error, language) };
    }
    if (!videos.length) return { say: said.nothing };
    remember(device, videos);
    // Пока колонка говорит «Включаю…», первый трек уже качается.
    audio.prepare(videos[0].id);
    return { say: said.playing(videos[0].title), play: videos.map(item) };
}

/** «Добавь в очередь …»: после того, что играет, и после добавленного раньше. */
async function add(query, device, { mentioned }) {
    const queue = queueOf(device);
    // «Добавь в очередь» без «ютуба», когда YouTube не играет, — не наше: пусть решает шлюз (Яндекс, своя музыка).
    if (!mentioned && !queue.playing) return null;
    const language = await locale();
    const said = words(language);
    const wanted = cleanQuery(query);
    if (!wanted) return { say: said.ask };
    let videos;
    try {
        videos = await find(wanted, language, { withMix: false });
    } catch (error) {
        return { say: sayError(error, language) };
    }
    if (!videos.length) return { say: said.nothing };
    const video = videos[0];
    // Как у шлюза: после текущего и после добавленных раньше — тогда «следующий заранее» угадывает верно.
    const at = Math.max(queue.ids.indexOf(queue.current) + 1, queue.addedUntil);
    queue.ids.splice(at, 0, video.id);
    queue.addedUntil = at + 1;
    if (device) lastDevice = device;
    return { say: said.added(video.title), add: [item(video)] };
}

const calls = {
    /** Фраза «включи с ютуба {query}». */
    playPhrase: (input) => play(input?.query, input?.request?.device),
    /** Фраза «добавь в очередь {query}». */
    addPhrase: (input) =>
        add(input?.query, input?.request?.device, { mentioned: /ютуб|youtube|you tube/i.test(input?.request?.text ?? input?.query ?? '') }),
    /** Инструмент «Мозга»: найти, не включая. */
    async searchTool(input) {
        const language = await locale();
        try {
            const found = await search(cleanQuery(input?.query), { language });
            const list = found.filter((video) => video.durationMs !== null).slice(0, 8);
            if (!list.length) return words(language).nothing;
            const lines = list.map((video) => `«${video.title}» — ${video.channel}, ${Math.round(video.durationMs / 60000)} мин`);
            return `${lines.join('\n')}\nВключить — youtube_play с названием; в очередь — youtube_add.`;
        } catch (error) {
            return sayError(error, language);
        }
    },
    /** Инструмент «Мозга»: включить. */
    playTool: (input) => play(input?.query, undefined),
    /** Инструмент «Мозга»: в очередь. */
    addTool: (input) => add(input?.query, undefined, { mentioned: true }),
    /** Источник музыки для плеера шлюза: по словам — найденное и похожее; без слов — нечего предложить. */
    async source(input) {
        const query = cleanQuery(input?.query);
        if (!query) return [];
        try {
            const videos = await find(query, await locale());
            return videos.slice(0, Math.max(1, Math.min(Number(input?.limit) || 25, 25))).map(item);
        } catch (error) {
            log(`источник «${query}»: ${error.message}`, 'warn');
            return [];
        }
    },
    /** Что плеер шлюза делает с нашими треками: играет ли YouTube, и что делать, если трек не заиграл. */
    async onPlayer(input) {
        const { name, payload } = input ?? {};
        const device = payload?.device;
        const id = payload?.item?.id;
        if (!device || !VIDEO_ID.test(id ?? '')) return null;
        const queue = queueOf(device);
        queue.at = Date.now();
        if (name === 'player.started') {
            queue.playing = true;
            queue.current = id;
            queue.skips = 0;
            return null;
        }
        if (name === 'player.stopped') {
            queue.playing = false;
            return null;
        }
        if (name !== 'player.failed' && !(name === 'player.silent' && !payload?.retrying)) return null;
        queue.playing = false;
        const error = audio.tracks.get(id)?.error ?? new YouTubeError('колонка не заиграла трек');
        const kind = kindOf(error);
        await sayLater(error, device);
        // Нет сети, YouTube не пускает, нет ffmpeg — следующий трек не заиграет тоже.
        if (['blocked', 'busy', 'ffmpeg'].includes(kind) || queue.skips >= SKIPS_IN_A_ROW) return null;
        const rest = queue.ids.slice(queue.ids.indexOf(id) + 1);
        if (!rest.length) return null;
        queue.skips += 1;
        log(`${id} не заиграл (${kind}): дальше по очереди`, 'warn');
        return { play: rest.map((next) => ({ id: next, title: next, stream: `${next}.mp3`, format: 'MP3', type: 'Track' })) };
    },
    /** Для окна и проверки: что стоит и что в кэше. */
    async status() {
        return {
            ytdlp: await ytdlp.version(),
            ffmpeg: audio.ffmpeg,
            tracks: [...audio.tracks.values()].map((track) => ({ id: track.id, state: track.state, size: track.size })),
            queues: Object.fromEntries([...queues].map(([device, queue]) => [device, { ...queue }])),
        };
    },
};

async function body(request) {
    let text = '';
    for await (const chunk of request) text += chunk;
    return text ? JSON.parse(text) : null;
}

const server = http.createServer(async (request, response) => {
    const url = new URL(request.url ?? '/', 'http://localhost');
    if (request.method === 'GET' && url.pathname === '/health') {
        response.writeHead(200).end('ok');
        return;
    }
    // Звук колонке: шлюз пересылает сюда `/media/apps/youtube/<id>.mp3`, токена у колонки нет.
    const sound = /^\/stream\/([A-Za-z0-9_-]{11})\.mp3$/.exec(url.pathname);
    if ((request.method === 'GET' || request.method === 'HEAD') && sound) {
        await audio.serve(request, response, sound[1]).catch((error) => {
            log(`поток ${sound[1]}: ${error.message}`, 'warn');
            if (!response.headersSent) response.writeHead(500);
            response.end();
        });
        return;
    }
    const match = request.method === 'POST' ? /^\/call\/([A-Za-z0-9_]+)$/.exec(url.pathname) : null;
    const call = match && Object.hasOwn(calls, match[1]) ? calls[match[1]] : null;
    if (!call) {
        response.writeHead(404).end();
        return;
    }
    if (request.headers.authorization !== `Bearer ${DUOWAVE_APP_TOKEN}`) {
        response.writeHead(401).end();
        return;
    }
    try {
        const result = await call(await body(request));
        response.writeHead(200, { 'content-type': 'application/json' }).end(JSON.stringify(result ?? null));
    } catch (error) {
        log(`${match[1]}: ${error?.stack ?? error}`, 'error');
        response.writeHead(400, { 'content-type': 'application/json' }).end(JSON.stringify({ error: sayError(error, await locale()) }));
    }
});

if (PORT) {
    server.listen(Number(PORT), '127.0.0.1', () => log(`YouTube слушает ${PORT}`));
    // yt-dlp — заранее, чтобы первая песня не ждала скачивания программы (около 35 МБ).
    ytdlp.path().then(
        () => ytdlp.version().then((version) => log(`yt-dlp: ${version}`)),
        (error) => log(`yt-dlp не скачался: ${error.message}`, 'warn'),
    );
}
