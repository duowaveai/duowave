// Звук видео для колонки: yt-dlp берёт дорожку (Opus или AAC), ffmpeg на компьютере делает из неё MP3 128 кбит/с —
// так колонка точно играет: AAC она не играет совсем (07.11, «Радио Рекорд»), Opus/WebM её плеером не проверен.
// Файл кладётся в кэш приложения и отдаётся колонке ещё во время записи; готовый — целиком и с Range, так «продолжи»
// после паузы идёт с места. Пока yt-dlp разгоняется (несколько секунд), колонке идёт тишина в том же MP3: её плеер
// начинает играть сразу, шлюз не считает колонку молчащей, а музыка встаёт за тишиной без шва.
import { spawn } from 'node:child_process';
import { createReadStream, promises as fs, existsSync } from 'node:fs';
import path from 'node:path';

import { kindOf } from './words.mjs';

/** Номер видео YouTube: 11 знаков из букв, цифр, `-` и `_`. */
export const VIDEO_ID = /^[A-Za-z0-9_-]{11}$/;

const KEEP_FILES = 40;
const AT_ONCE = 2;
// Сколько ждать первых байт молча, прежде чем пустить тишину; и сколько ждать файла целиком для Range с середины.
const QUIET_MS = 1200;
const WHOLE_MS = 120_000;
const RUN_LIMIT_MS = 5 * 60_000;
const POLL_MS = 150;

const MP3 = ['-c:a', 'libmp3lame', '-b:a', '128k', '-ar', '44100', '-ac', '2', '-id3v2_version', '0', '-write_xing', '0', '-f', 'mp3'];

/** Где ffmpeg: `FFMPEG`, PATH, обычные места Homebrew и системы. */
export function findFfmpeg(env = process.env) {
    const name = process.platform === 'win32' ? 'ffmpeg.exe' : 'ffmpeg';
    const fromPath = (env.PATH ?? '').split(path.delimiter).filter(Boolean).map((dir) => path.join(dir, name));
    const usual = ['/opt/homebrew/bin/ffmpeg', '/usr/local/bin/ffmpeg', '/usr/bin/ffmpeg'];
    return [env.FFMPEG, ...fromPath, ...usual].find((candidate) => candidate && existsSync(candidate)) ?? null;
}

/** Заголовок Range: `bytes=<с>-<по>` → {start, end}; иное — null. */
export function parseRange(header, size) {
    const match = /^bytes=(\d*)-(\d*)$/.exec(String(header ?? '').trim());
    if (!match || (match[1] === '' && match[2] === '')) return null;
    let start;
    let end;
    if (match[1] === '') {
        start = Math.max(0, size - Number(match[2]));
        end = size - 1;
    } else {
        start = Number(match[1]);
        end = match[2] === '' ? size - 1 : Math.min(Number(match[2]), size - 1);
    }
    if (start >= size || start > end) return { start, end, unsatisfiable: true };
    return { start, end };
}

class Track {
    constructor(id, file) {
        this.id = id;
        this.file = file;
        this.part = `${file}.part`;
        this.state = 'waiting';
        this.error = null;
        this.size = 0;
        this.listeners = new Set();
        this.done = new Promise((resolve) => {
            this.finish = resolve;
        });
    }

    tell() {
        for (const listener of this.listeners) listener();
    }

    /** Ждать перемен (новые байты, конец, ошибка), но не дольше `ms`. */
    change(ms) {
        return new Promise((resolve) => {
            const done = () => {
                clearTimeout(timer);
                this.listeners.delete(done);
                resolve();
            };
            const timer = setTimeout(done, Math.max(0, ms));
            this.listeners.add(done);
        });
    }
}

export class Audio {
    /**
     * @param {object} options
     * @param {string} options.dir папка кэша звука
     * @param {{path(): Promise<string>, broken(): Promise<unknown>}} options.ytdlp
     * @param {(line: string) => void} [options.log]
     * @param {(id: string) => void} [options.onServe] колонка взяла трек: приготовить следующий
     */
    constructor({ dir, ytdlp, log = () => {}, onServe = () => {} }) {
        this.dir = dir;
        this.ytdlp = ytdlp;
        this.log = log;
        this.onServe = onServe;
        this.tracks = new Map();
        this.running = 0;
        this.waiting = [];
        this.silence = null;
        this.ffmpeg = findFfmpeg();
    }

    #file(id) {
        return path.join(this.dir, `${id}.mp3`);
    }

    /** Трек готовится (или уже готов): yt-dlp и ffmpeg запускаются не больше двух сразу. */
    prepare(id) {
        if (!VIDEO_ID.test(id)) throw new Error(`не номер видео: ${id}`);
        const known = this.tracks.get(id);
        if (known && known.state !== 'failed') return known;
        const track = new Track(id, this.#file(id));
        this.tracks.set(id, track);
        if (existsSync(track.file)) {
            track.state = 'done';
            track.finish();
            fs.stat(track.file).then((stat) => {
                track.size = stat.size;
                fs.utimes(track.file, new Date(), new Date()).catch(() => {});
            });
            return track;
        }
        this.waiting.push(track);
        this.#next();
        return track;
    }

    #next() {
        while (this.running < AT_ONCE && this.waiting.length) {
            const track = this.waiting.shift();
            this.running += 1;
            this.#run(track, true)
                .catch((error) => this.#fail(track, error))
                .finally(() => {
                    this.running -= 1;
                    this.#next();
                });
        }
    }

    #fail(track, error) {
        track.state = 'failed';
        track.error = error;
        track.finish();
        track.tell();
        fs.rm(track.part, { force: true }).catch(() => {});
    }

    async #run(track, retry) {
        if (!this.ffmpeg) this.ffmpeg = findFfmpeg();
        if (!this.ffmpeg) throw Object.assign(new Error('нет ffmpeg'), { kind: 'ffmpeg' });
        const ytdlp = await this.ytdlp.path();
        await fs.mkdir(this.dir, { recursive: true });
        track.state = 'preparing';
        const started = Date.now();
        // Первый трек ждёт дольше остальных: YouTube отдаёт звук только после того, как рекламу перед видео можно
        // пропустить (yt-dlp честно ждёт 5–6 с), — следующие готовятся заранее, пока играет текущий.
        const { ok, stderr } = await this.#pipe(ytdlp, track);
        if (ok && track.size > 0) {
            await fs.rename(track.part, track.file);
            track.state = 'done';
            track.finish();
            track.tell();
            this.log(`${track.id}: готов за ${((Date.now() - started) / 1000).toFixed(1)} с, ${track.size} байт`);
            this.#evict();
            return;
        }
        const error = new Error(stderr.trim().split('\n').slice(-3).join(' ') || 'yt-dlp не дал звука');
        // YouTube сменил шифр: новый yt-dlp — и ещё одна попытка, если он нашёлся.
        if (retry && kindOf(error) === 'cipher' && track.size === 0) {
            const updated = await this.ytdlp.broken();
            if (updated?.updated) {
                this.log(`${track.id}: yt-dlp обновлён до ${updated.tag}, пробую снова`);
                return this.#run(track, false);
            }
        }
        throw error;
    }

    /** yt-dlp → ffmpeg → файл; размер растёт — слушатели узнают. */
    #pipe(ytdlp, track) {
        return new Promise((resolve) => {
            const url = `https://www.youtube.com/watch?v=${track.id}`;
            const cache = path.join(path.dirname(this.dir), 'yt-dlp-cache');
            const fetcher = spawn(
                ytdlp,
                [
                    '--quiet', '--no-warnings', '--no-progress', '--no-playlist',
                    '--format', 'bestaudio[acodec=opus]/bestaudio/best',
                    // YouTube проверяет клиента задачкой на JS: решает её Node, на котором работает само приложение.
                    '--js-runtimes', `node:${process.execPath}`,
                    '--cache-dir', cache,
                    '--output', '-', '--', url,
                ],
                { stdio: ['ignore', 'pipe', 'pipe'] },
            );
            const encoder = spawn(this.ffmpeg, ['-hide_banner', '-loglevel', 'error', '-i', 'pipe:0', '-vn', ...MP3, '-y', track.part], {
                stdio: ['pipe', 'ignore', 'pipe'],
            });
            let stderr = '';
            fetcher.stderr.on('data', (chunk) => {
                stderr = (stderr + chunk).slice(-4000);
            });
            encoder.stderr.on('data', (chunk) => {
                stderr = (stderr + chunk).slice(-4000);
            });
            fetcher.stdout.pipe(encoder.stdin);
            encoder.stdin.on('error', () => {});
            const watch = setInterval(async () => {
                const size = await fs.stat(track.part).then((stat) => stat.size, () => 0);
                if (size !== track.size) {
                    track.size = size;
                    track.tell();
                }
            }, POLL_MS);
            const limit = setTimeout(() => {
                stderr += '\nслишком долго';
                fetcher.kill('SIGKILL');
                encoder.kill('SIGKILL');
            }, RUN_LIMIT_MS);
            const codes = [];
            const exited = (code) => {
                codes.push(code);
                if (codes.length < 2) return;
                clearInterval(watch);
                clearTimeout(limit);
                fs.stat(track.part).then(
                    (stat) => {
                        track.size = stat.size;
                        resolve({ ok: codes.every((code) => code === 0), stderr });
                    },
                    () => resolve({ ok: false, stderr }),
                );
            };
            fetcher.on('error', (error) => {
                stderr += `\n${error.message}`;
            });
            encoder.on('error', (error) => {
                stderr += `\n${error.message}`;
            });
            fetcher.on('close', exited);
            encoder.on('close', exited);
        });
    }

    /** Старые файлы кэша уходят: остаются последние KEEP_FILES. */
    async #evict() {
        const names = (await fs.readdir(this.dir).catch(() => [])).filter((name) => name.endsWith('.mp3'));
        if (names.length <= KEEP_FILES) return;
        const dated = await Promise.all(
            names.map(async (name) => ({ name, at: (await fs.stat(path.join(this.dir, name)).catch(() => null))?.mtimeMs ?? 0 })),
        );
        dated.sort((a, b) => a.at - b.at);
        for (const { name } of dated.slice(0, dated.length - KEEP_FILES)) {
            const id = name.slice(0, -4);
            if (this.tracks.get(id)?.state === 'preparing') continue;
            this.tracks.delete(id);
            await fs.rm(path.join(this.dir, name), { force: true });
        }
    }

    /** Секунда тишины MP3 с теми же настройками, что у треков: склеивается с ними без шва. */
    async #silence() {
        if (this.silence) return this.silence;
        if (!this.ffmpeg) return null;
        const chunks = [];
        const made = await new Promise((resolve) => {
            const child = spawn(this.ffmpeg, ['-hide_banner', '-loglevel', 'error', '-f', 'lavfi', '-i', 'anullsrc=r=44100:cl=stereo', '-t', '1', ...MP3, 'pipe:1'], {
                stdio: ['ignore', 'pipe', 'ignore'],
            });
            child.stdout.on('data', (chunk) => chunks.push(chunk));
            child.on('close', (code) => resolve(code === 0));
            child.on('error', () => resolve(false));
        });
        this.silence = made ? Buffer.concat(chunks) : null;
        return this.silence;
    }

    /** `GET /stream/<id>.mp3`: целиком, с Range, или по мере записи. */
    async serve(request, response, id) {
        let track;
        try {
            track = this.prepare(id);
        } catch {
            response.writeHead(404).end();
            return;
        }
        this.onServe(id);
        const range = request.headers.range;
        if (track.state !== 'done' && range && !/^bytes=0-\d*$/.test(range.trim())) {
            // С середины — только по готовому файлу: его длина и место известны.
            await Promise.race([track.done, new Promise((resolve) => setTimeout(resolve, WHOLE_MS))]);
        }
        if (track.state === 'done') return this.#whole(request, response, track);
        if (track.state === 'failed') return failed(response, track.error);
        await this.#growing(response, track);
    }

    async #whole(request, response, track) {
        const stat = await fs.stat(track.file).catch(() => null);
        if (!stat) {
            this.tracks.delete(track.id);
            response.writeHead(404).end();
            return;
        }
        const headers = { 'content-type': 'audio/mpeg', 'accept-ranges': 'bytes', 'cache-control': 'no-store' };
        const range = parseRange(request.headers.range, stat.size);
        if (range?.unsatisfiable) {
            response.writeHead(416, { 'content-range': `bytes */${stat.size}` }).end();
            return;
        }
        if (range) {
            response.writeHead(206, {
                ...headers,
                'content-range': `bytes ${range.start}-${range.end}/${stat.size}`,
                'content-length': range.end - range.start + 1,
            });
            createReadStream(track.file, { start: range.start, end: range.end }).pipe(response);
            return;
        }
        response.writeHead(200, { ...headers, 'content-length': stat.size });
        if (request.method === 'HEAD') return response.end();
        createReadStream(track.file).pipe(response);
    }

    /** По мере записи: сначала, если байт ещё нет, — тишина; потом файл с начала до конца записи. */
    async #growing(response, track) {
        let closed = false;
        response.on('close', () => {
            closed = true;
        });
        const quietUntil = Date.now() + QUIET_MS;
        while (track.size === 0 && track.state !== 'failed' && track.state !== 'done' && Date.now() < quietUntil) {
            await track.change(quietUntil - Date.now());
        }
        if (track.state === 'failed') return failed(response, track.error);
        response.writeHead(200, { 'content-type': 'audio/mpeg', 'cache-control': 'no-store' });
        const silence = track.size === 0 && track.state !== 'done' ? await this.#silence() : null;
        let paddedUntil = Date.now();
        const busy = () => track.state === 'waiting' || track.state === 'preparing';
        while (silence && !closed && track.size === 0 && busy()) {
            if (Date.now() >= paddedUntil - 500) {
                response.write(silence);
                paddedUntil = Math.max(paddedUntil, Date.now()) + 1000;
            }
            await track.change(250);
        }
        if (track.state === 'failed' || closed) {
            response.end();
            return;
        }
        let offset = 0;
        let handle = null;
        try {
            // Дописанный файл переименовывается из `.part`: открытый раньше читается и после.
            handle = await fs.open(track.part, 'r').catch(() => fs.open(track.file, 'r'));
            const buffer = Buffer.alloc(64 * 1024);
            while (!closed) {
                const available = track.size - offset;
                if (available > 0) {
                    const { bytesRead } = await handle.read(buffer, 0, Math.min(buffer.length, available), offset);
                    if (bytesRead === 0) {
                        await track.change(POLL_MS);
                        continue;
                    }
                    offset += bytesRead;
                    if (!response.write(buffer.subarray(0, bytesRead))) {
                        await new Promise((resolve) => {
                            const done = () => {
                                response.off('drain', done);
                                response.off('close', done);
                                resolve();
                            };
                            response.on('drain', done);
                            response.on('close', done);
                        });
                    }
                    continue;
                }
                if (track.state === 'done' || track.state === 'failed') break;
                await track.change(1000);
            }
        } catch (error) {
            this.log(`${track.id}: отдать не вышло — ${error.message}`);
        } finally {
            await handle?.close().catch(() => {});
            response.end();
        }
    }
}

function failed(response, error) {
    if (response.headersSent) return response.end();
    response.writeHead(502, { 'content-type': 'text/plain; charset=utf-8' }).end(String(error?.message ?? error));
}
