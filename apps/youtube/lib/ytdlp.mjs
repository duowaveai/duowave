// yt-dlp (Unlicense) — то, что достаёт звук из YouTube. Решение 6 плана speaker-alone: приложение само скачивает его
// с GitHub при первом запуске, сверяет SHA-256 со списком сумм того же выпуска и само обновляет — раз в сутки и сразу,
// когда YouTube сменил шифр и yt-dlp перестал справляться. Так YouTube чинится в тот же день, без нашего выпуска.
//
// На Mac — сборка-папка (`yt-dlp_macos.zip`): однофайловая распаковывает себя при каждом запуске, и macOS проверяет
// распакованное заново — около 10 с на каждый трек. Папка запускается за десятые доли секунды; первый, долгий запуск
// (проверка macOS новой программы) делается сразу при установке, не на первой песне.
import { execFile } from 'node:child_process';
import { createHash } from 'node:crypto';
import { promises as fs } from 'node:fs';
import path from 'node:path';
import { promisify } from 'node:util';

const run = promisify(execFile);
const RELEASES = 'https://github.com/yt-dlp/yt-dlp/releases';
const DAY_MS = 24 * 3600 * 1000;
// После «YouTube сменил шифр» новую версию ищем не чаще раза в час: выпуск yt-dlp выходит через часы, не минуты.
const BROKEN_RETRY_MS = 3600 * 1000;
const DOWNLOAD_TIMEOUT_MS = 10 * 60 * 1000;

/**
 * Файл выпуска для этой системы и что в нём запускать: `{asset, zip, program}`; null — сборки yt-dlp для неё нет.
 * `zip` — папка в архиве (распаковывается системным `ditto`), иначе — один исполняемый файл.
 */
export function assetFor(platform = process.platform, arch = process.arch) {
    if (platform === 'darwin') return { asset: 'yt-dlp_macos.zip', zip: true, program: 'yt-dlp_macos' };
    if (platform === 'win32') return { asset: arch === 'arm64' ? 'yt-dlp_arm64.exe' : 'yt-dlp.exe', zip: false, program: 'yt-dlp.exe' };
    if (platform === 'linux' && arch === 'x64') return { asset: 'yt-dlp_linux', zip: false, program: 'yt-dlp' };
    if (platform === 'linux' && arch === 'arm64') return { asset: 'yt-dlp_linux_aarch64', zip: false, program: 'yt-dlp' };
    return null;
}

/** Сумма файла из SHA2-256SUMS выпуска: строки «<hex>  <имя>». */
export function sumFor(sums, asset) {
    for (const line of sums.split('\n')) {
        const [hex, name] = line.trim().split(/\s+/);
        if (name === asset && /^[0-9a-f]{64}$/i.test(hex ?? '')) return hex.toLowerCase();
    }
    return null;
}

/** Номер последнего выпуска: GitHub отвечает на /latest переходом на /tag/<номер>. */
async function latestTag() {
    const answer = await fetch(`${RELEASES}/latest`, { redirect: 'manual', signal: AbortSignal.timeout(15000) });
    const where = answer.headers.get('location') ?? '';
    const tag = where.split('/tag/')[1];
    if (!tag || !/^[\w.-]+$/.test(tag)) throw new Error(`GitHub не назвал выпуск yt-dlp (HTTP ${answer.status})`);
    return tag;
}

async function download(url, timeoutMs) {
    const answer = await fetch(url, { signal: AbortSignal.timeout(timeoutMs) });
    if (!answer.ok) throw new Error(`GitHub ответил HTTP ${answer.status} на ${url.split('/').pop()}`);
    return Buffer.from(await answer.arrayBuffer());
}

const exists = (file) => fs.stat(file).then(() => true, () => false);

export class YtDlp {
    /**
     * @param {string} dir папка приложения для программы (`<DUOWAVE_APP_DATA>/bin`)
     * @param {(line: string) => void} log
     */
    constructor(dir, log = () => {}) {
        this.dir = dir;
        this.log = log;
        this.build = assetFor();
        this.state = path.join(dir, 'yt-dlp.json');
        this.installing = null;
        this.lastBroken = 0;
    }

    async #read() {
        try {
            return JSON.parse(await fs.readFile(this.state, 'utf8'));
        } catch {
            return null;
        }
    }

    /** Путь к yt-dlp: свой из `DUOWAVE_YTDLP` (для разработки), иначе скачанный; нет — скачать сейчас. */
    async path() {
        if (process.env.DUOWAVE_YTDLP) return process.env.DUOWAVE_YTDLP;
        const known = await this.#read();
        if (known?.tag && known.program && (await exists(known.program))) {
            if (Date.now() - (known.checkedAt ?? 0) > DAY_MS) this.update().catch(() => {});
            return known.program;
        }
        await this.install();
        return (await this.#read()).program;
    }

    /** Версия, что стоит: номер выпуска или null. */
    async version() {
        if (process.env.DUOWAVE_YTDLP) return 'свой (DUOWAVE_YTDLP)';
        return (await this.#read())?.tag ?? null;
    }

    /** Поставить последний выпуск, если он новее того, что стоит. Один раз за раз: второй зов ждёт первый. */
    install() {
        this.installing ??= this.#install().finally(() => {
            this.installing = null;
        });
        return this.installing;
    }

    async #install() {
        if (!this.build) throw new Error(`yt-dlp не собирается под ${process.platform}-${process.arch}`);
        const { asset, zip, program } = this.build;
        const tag = await latestTag();
        const known = await this.#read();
        if (known?.tag === tag && known.program && (await exists(known.program))) {
            await fs.writeFile(this.state, JSON.stringify({ ...known, checkedAt: Date.now() }));
            return { tag, updated: false };
        }
        const base = `${RELEASES}/download/${tag}`;
        const sums = (await download(`${base}/SHA2-256SUMS`, 30000)).toString('utf8');
        const expected = sumFor(sums, asset);
        if (!expected) throw new Error(`в SHA2-256SUMS выпуска ${tag} нет ${asset}`);
        this.log(`yt-dlp ${tag}: скачиваю ${asset}`);
        const body = await download(`${base}/${asset}`, DOWNLOAD_TIMEOUT_MS);
        const actual = createHash('sha256').update(body).digest('hex');
        if (actual !== expected) throw new Error(`yt-dlp ${tag}: SHA-256 не сходится (${actual} ≠ ${expected})`);
        // Каждая версия — своя папка: идущая песня доигрывает старой, следующая берёт новую.
        const home = path.join(this.dir, `yt-dlp-${tag}`);
        const part = `${home}.part`;
        await fs.rm(part, { recursive: true, force: true });
        await fs.mkdir(part, { recursive: true });
        if (zip) {
            const archive = path.join(this.dir, `${asset}.part`);
            await fs.writeFile(archive, body);
            try {
                await run('/usr/bin/ditto', ['-x', '-k', archive, part]);
            } finally {
                await fs.rm(archive, { force: true });
            }
        } else {
            await fs.writeFile(path.join(part, program), body, { mode: 0o755 });
        }
        await fs.rm(home, { recursive: true, force: true });
        await fs.rename(part, home);
        const file = path.join(home, program);
        // Первый запуск новой программы macOS проверяет долго — пусть это будет сейчас, а не на песне.
        const warm = Date.now();
        const { stdout } = await run(file, ['--version'], { timeout: 120_000 });
        this.log(`yt-dlp ${stdout.trim()}: поставлен, SHA-256 сходится, первый запуск ${((Date.now() - warm) / 1000).toFixed(1)} с`);
        await fs.writeFile(this.state, JSON.stringify({ tag, sha256: actual, program: file, checkedAt: Date.now() }));
        // Старые версии уходят; прежняя остаётся до следующего обновления — ею может доигрывать идущая песня.
        const keep = new Set([path.basename(home), known?.program && path.basename(path.dirname(known.program))]);
        for (const name of await fs.readdir(this.dir)) {
            if (name.startsWith('yt-dlp-') && !keep.has(name)) {
                await fs.rm(path.join(this.dir, name), { recursive: true, force: true });
            }
        }
        return { tag, updated: true };
    }

    /** Проверка новой версии (раз в сутки). Ошибки — в журнал: старая версия продолжает работать. */
    async update() {
        try {
            return await this.install();
        } catch (error) {
            this.log(`yt-dlp: обновить не вышло — ${error.message}`);
            return null;
        }
    }

    /** YouTube сменил шифр, yt-dlp не справился: поискать новый выпуск, но не чаще раза в час. */
    broken() {
        if (process.env.DUOWAVE_YTDLP || Date.now() - this.lastBroken < BROKEN_RETRY_MS) return Promise.resolve(null);
        this.lastBroken = Date.now();
        return this.update();
    }
}
