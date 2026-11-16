// Поиск и «похожее» YouTube — открытым API сайта (InnerTube), без ключа и без входа: это те же запросы, что делает
// страница youtube.com. Звук здесь не берётся — его достаёт yt-dlp (ytdlp.mjs): адрес звука зашифрован, и шифр YouTube
// меняет часто. Поиск и микс (`RD<id>`) отдаются простым JSON, их хватает своего кода.

const BASE = 'https://www.youtube.com/youtubei/v1';
// Версия клиента сайта: YouTube принимает и старые, свежая нужна только для вида ответа.
const CLIENT_VERSION = '2.20251110.01.00';
// Фильтр поиска «только видео» — без каналов, плейлистов и подборок.
const ONLY_VIDEOS = 'EgIQAQ%3D%3D';
const TIMEOUT_MS = 8000;

/** Ошибка YouTube с HTTP-кодом: 429 — «слишком много запросов с адреса», остальное — сеть или сам YouTube. */
export class YouTubeError extends Error {
    constructor(message, { status = 0, network = false } = {}) {
        super(message);
        this.status = status;
        this.network = network;
    }
}

/** «4:45», «1:02:03» → миллисекунды; без длины (прямой эфир) — null. */
export function durationMs(text) {
    if (typeof text !== 'string' || !/^\d+(:\d{1,2}){1,2}$/.test(text.trim())) return null;
    return text.trim().split(':').reduce((total, part) => total * 60 + Number(part), 0) * 1000;
}

/** Текст поля YouTube: `{simpleText}` или `{runs: [{text}]}`. */
function text(field) {
    if (!field) return '';
    if (typeof field.simpleText === 'string') return field.simpleText;
    if (Array.isArray(field.runs)) return field.runs.map((run) => run.text ?? '').join('');
    return '';
}

/** Обложка по номеру видео: всегда есть и не зависит от вида ответа. */
export function cover(id) {
    return `https://i.ytimg.com/vi/${id}/hqdefault.jpg`;
}

/** Видео из ответа поиска: `videoRenderer` в разделах результата. */
export function parseSearch(answer) {
    const sections =
        answer?.contents?.twoColumnSearchResultsRenderer?.primaryContents?.sectionListRenderer?.contents ?? [];
    const found = [];
    for (const section of sections) {
        for (const entry of section?.itemSectionRenderer?.contents ?? []) {
            const video = entry?.videoRenderer;
            if (!video?.videoId) continue;
            found.push({
                id: video.videoId,
                title: text(video.title).trim(),
                channel: text(video.ownerText ?? video.longBylineText ?? video.shortBylineText).trim(),
                durationMs: durationMs(text(video.lengthText)),
            });
        }
    }
    return found.filter((video) => video.title);
}

/** Видео из микса: панель плейлиста на странице просмотра (`playlistPanelVideoRenderer`). */
export function parseMix(answer) {
    const contents = answer?.contents?.twoColumnWatchNextResults?.playlist?.playlist?.contents ?? [];
    const found = [];
    for (const entry of contents) {
        const video = entry?.playlistPanelVideoRenderer;
        if (!video?.videoId) continue;
        found.push({
            id: video.videoId,
            title: text(video.title).trim(),
            channel: text(video.longBylineText ?? video.shortBylineText).trim(),
            durationMs: durationMs(text(video.lengthText)),
        });
    }
    return found.filter((video) => video.title);
}

async function ask(endpoint, body, language) {
    const context = { client: { clientName: 'WEB', clientVersion: CLIENT_VERSION, hl: language || 'en' } };
    let answer;
    try {
        answer = await fetch(`${BASE}/${endpoint}?prettyPrint=false`, {
            method: 'POST',
            headers: { 'content-type': 'application/json' },
            body: JSON.stringify({ context, ...body }),
            signal: AbortSignal.timeout(TIMEOUT_MS),
        });
    } catch (error) {
        throw new YouTubeError(`YouTube не ответил: ${error?.cause?.code ?? error?.name ?? error}`, { network: true });
    }
    if (!answer.ok) throw new YouTubeError(`YouTube ответил HTTP ${answer.status}`, { status: answer.status });
    return answer.json();
}

/** Поиск видео по словам. */
export async function search(query, { language } = {}) {
    return parseSearch(await ask('search', { query, params: ONLY_VIDEOS }, language));
}

/** Микс YouTube для видео (`RD<id>`): само видео первым, дальше похожее. */
export async function mix(id, { language } = {}) {
    return parseMix(await ask('next', { videoId: id, playlistId: `RD${id}` }, language));
}

// Длинное (сборник, концерт целиком) не берём первым, если человек не просил его словами.
const LONG_MS = 15 * 60 * 1000;
const WANTS_LONG = /(альбом|сборник|концерт|микс|подборк|mix|album|full|live|playlist|плейлист)/i;

/** Что включить из найденного: первое не слишком длинное и не прямой эфир. */
export function pick(found, query = '') {
    const long = WANTS_LONG.test(query);
    return (
        found.find((video) => video.durationMs !== null && (long || video.durationMs <= LONG_MS)) ??
        found.find((video) => video.durationMs !== null) ??
        null
    );
}
