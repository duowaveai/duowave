// Перенос приложений на витрину сайта (docs/plans/web-host.md): собирает веб-хозяина и кладёт рядом пакеты, которые
// прошли проверку (src/web-host/check.ts). Не прошёл — на витрину не идёт, скрипт называет почему и кончает с кодом 1.
// node ≥ 23.6 запускает файл как есть:
//
//     node panel/scripts/web-tools.ts <папка витрины> <папка пакета>…
//
// В папке витрины появятся web-host/ (хозяин: index.html, frame.html, assets/, sdk/) и apps/ (apps/<id>/ — manifest.json,
// backend.js, panel.js, panel.css и apps/index.json — список). Обе папки пишутся заново целиком: прежние стираются, но
// только если это и правда папки витрины (в них есть index.html или index.json) или их нет.

import { spawnSync } from 'node:child_process';
import { cpSync, existsSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import { checkPackage, mainSection } from '../src/web-host/check.ts';
import type { ShelfEntry } from '../src/web-host/host.ts';

const PANEL = join(dirname(fileURLToPath(import.meta.url)), '..');
const SHIPPED = ['manifest.json', 'backend.js', 'panel.js', 'panel.css'];

function usage(): never {
    console.error('node panel/scripts/web-tools.ts <папка витрины> <папка пакета>…');
    process.exit(2);
}

/** Стереть папку витрины — только свою: с меткой или пустую. */
function clear(folder: string, mark: string): void {
    if (!existsSync(folder)) return;
    const names = readdirSync(folder);
    if (names.length > 0 && !names.includes(mark)) {
        console.error(`${folder}: не похоже на папку витрины (нет ${mark}) — не трогаю.`);
        process.exit(2);
    }
    rmSync(folder, { recursive: true, force: true });
}

const [outArg, ...packages] = process.argv.slice(2);
if (!outArg || packages.length === 0) usage();
const out = resolve(outArg);
const hostOut = join(out, 'web-host');
const appsOut = join(out, 'apps');

// ——— Проверка пакетов: до сборки, чтобы не стирать витрину зря ———
const passed: Array<{ folder: string; entry: ShelfEntry }> = [];
let failed = 0;
for (const given of packages) {
    const folder = resolve(given);
    const read = (name: string): string | undefined => (existsSync(join(folder, name)) ? readFileSync(join(folder, name), 'utf8') : undefined);
    const manifest = read('manifest.json');
    if (manifest === undefined) {
        console.error(`✗ ${given}: нет manifest.json`);
        failed += 1;
        continue;
    }
    const names = readdirSync(folder, { withFileTypes: true }).map((entry) => (entry.isDirectory() ? `${entry.name}/` : entry.name));
    const checked = checkPackage({ manifest, backend: read('backend.js'), panel: read('panel.js'), names });
    if (!checked.ok) {
        console.error(`✗ ${checked.id || given}: на витрину не идёт`);
        for (const problem of checked.problems) console.error(`    — ${problem}`);
        failed += 1;
        continue;
    }
    const parsed = JSON.parse(manifest) as Record<string, unknown>;
    if (passed.some((item) => item.entry.id === checked.id)) {
        console.error(`✗ ${checked.id}: второй пакет с тем же id`);
        failed += 1;
        continue;
    }
    passed.push({
        folder,
        entry: { id: checked.id, version: String(parsed.version ?? '0'), section: mainSection(parsed) ?? checked.id, styles: names.includes('panel.css') },
    });
    console.log(`✓ ${checked.id} ${String(parsed.version ?? '')}${checked.only.length ? ` — только в DuoWave: ${checked.only.join(', ')}` : ''}`);
}

// ——— Хозяин ———
clear(hostOut, 'index.html');
const built = spawnSync('pnpm', ['exec', 'vite', 'build', '-c', 'vite.web-host.config.ts', '--logLevel', 'warn'], {
    cwd: PANEL,
    stdio: 'inherit',
    env: { ...process.env, WEB_HOST_OUT: hostOut },
});
if (built.status !== 0) {
    console.error('Сборка веб-хозяина не удалась.');
    process.exit(1);
}

// ——— Приложения ———
clear(appsOut, 'index.json');
mkdirSync(appsOut, { recursive: true });
for (const { folder, entry } of passed) {
    const to = join(appsOut, entry.id);
    mkdirSync(to, { recursive: true });
    for (const name of SHIPPED) if (existsSync(join(folder, name))) cpSync(join(folder, name), join(to, name));
}
writeFileSync(join(appsOut, 'index.json'), `${JSON.stringify(passed.map((item) => item.entry), null, 2)}\n`);
console.log(`Витрина: ${out} — хозяин и ${passed.length} прил.${failed ? `; не прошли проверку: ${failed}` : ''}`);
process.exit(failed ? 1 : 0);
