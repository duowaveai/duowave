// Проверки веб-хозяина без браузера (docs/plans/web-host.md): проверка пакетов для витрины (src/web-host/check.ts) и
// `host` бэкенда в браузере (src/web-host/backend.ts) — на настоящих пакетах «Неоновая змейка» и шаблоне учителя.
// node ≥ 23.6 запускает файл как есть:
//
//     node panel/scripts/web-host-check.ts        (или pnpm --filter panel web-host-check)

import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

import { callBackend, defaultSettings, languagesOf, makeHost } from '../src/web-host/backend.ts';
import { checkPackage, codeOnly, exportsOf, importsOf, mainSection } from '../src/web-host/check.ts';

const checks: Array<[string, () => void | Promise<void>]> = [];
const check = (name: string, run: () => void | Promise<void>): void => void checks.push([name, run]);
function expect(condition: unknown, message: string): asserts condition {
    if (!condition) throw new Error(message);
}

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..', '..');
const read = (path: string): string => readFileSync(join(ROOT, path), 'utf8');

const SNAKE = {
    manifest: read('packages/neon-snake/manifest.json'),
    backend: read('packages/neon-snake/backend.js'),
    panel: read('packages/neon-snake/panel.js'),
    names: ['README.md', 'backend.js', 'manifest.json', 'panel.css', 'panel.js'],
};

/** Шаблон учителя, как его заполняет templates.rs (`fill`): id, название. Манифест — как у шлюза, по сути. */
function teacher(): { manifest: string; backend: string; panel: string; names: string[] } {
    const fill = (text: string): string => text.replace('"__TITLE__"', '"Piano teacher"').replaceAll('__TITLE__', 'Piano teacher').replaceAll('__ID__', 'piano-teacher');
    const manifest = {
        schema: 1,
        kind: 'plugin',
        id: 'piano-teacher',
        version: '0.1.0',
        title: 'Piano teacher',
        api: '^2',
        runs: ['pc', 'stationServer'],
        languages: ['ru', 'en'],
        permissions: ['listen:device/midi'],
        settings: [{ key: 'names', type: 'boolean', title: 'Подписывать ноты', default: true }],
        contributes: {
            panel: { slots: ['desk.widgets'], sections: [{ id: 'piano-teacher', title: 'Piano teacher' }], calls: ['state', 'choose', 'add', 'remove', 'keyboard', 'done'] },
            intents: [
                { phrases: ['давай учить {melody}'], handler: 'learn' },
                { phrases: ['какие мелодии есть'], handler: 'about' },
            ],
        },
    };
    return {
        manifest: JSON.stringify(manifest),
        backend: fill(read('crates/duowave-apps/src/plugins/workshop/templates/midi/backend.js')),
        panel: fill(read('crates/duowave-apps/src/plugins/workshop/templates/midi/panel.js')),
        names: ['README.md', 'backend.js', 'manifest.json', 'panel.js'],
    };
}

const withManifest = (base: { manifest: string }, change: (manifest: Record<string, unknown>) => void): string => {
    const manifest = JSON.parse(base.manifest) as Record<string, unknown>;
    change(manifest);
    return JSON.stringify(manifest);
};

// ─── The check: what goes to the site's shelf ────────────────────────────────────────────────────────────────────────

check('the snake goes to the shelf as it is', () => {
    const checked = checkPackage(SNAKE);
    expect(checked.ok, checked.problems.join('; '));
    // The package's own id (random since 20.11, p-<12 hex>), not its folder's name.
    expect(checked.id === (JSON.parse(SNAKE.manifest) as { id: string }).id, checked.id);
    expect(checked.only.length === 0, checked.only.join(', '));
});

check('the teacher goes, its voice phrases named as DuoWave-only', () => {
    const checked = checkPackage(teacher());
    expect(checked.ok, checked.problems.join('; '));
    expect(checked.only.some((line) => line.startsWith('голосовые фразы (2)')), checked.only.join(', '));
});

check('a right the shelf cannot give stops a package', () => {
    const manifest = withManifest(SNAKE, (m) => (m.permissions = ['net:api.example.com']));
    const checked = checkPackage({ ...SNAKE, manifest });
    expect(!checked.ok && checked.problems.some((p) => p.includes('net:api.example.com')), checked.problems.join('; '));
});

check('the gateway’s work in the manifest stops a package: schedules, the bus, cards', () => {
    for (const [key, value] of [
        ['schedules', [{ id: 'daily', at: '09:00', handler: 'x' }]],
        ['bus', { handles: [] }],
        ['activators', [{ id: 'a', word: 'b' }]],
    ] as const) {
        const manifest = withManifest(SNAKE, (m) => ((m.contributes as Record<string, unknown>)[key] = value));
        const checked = checkPackage({ ...SNAKE, manifest });
        expect(!checked.ok && checked.problems.some((p) => p.includes(`contributes.${key}`)), `${key}: ${checked.problems.join('; ')}`);
    }
    const cards = withManifest(SNAKE, (m) => (((m.contributes as Record<string, unknown>).panel as Record<string, unknown>).cards = [{ id: 'c' }]));
    expect(!checkPackage({ ...SNAKE, manifest: cards }).ok, 'cards need a renderer the shelf has not');
});

check('a card-only app (no panel.js) does not go', () => {
    const shopping = {
        manifest: read('packages/shopping/manifest.json'),
        backend: read('packages/shopping/backend.js'),
        names: ['backend.js', 'manifest.json'],
    };
    const checked = checkPackage(shopping);
    expect(!checked.ok && checked.problems.some((p) => p.includes('panel.js')), checked.problems.join('; '));
});

check('the backend may use the store, settings, language and log — not the network, bus, folders or tasks', () => {
    for (const call of ['host.http.fetch("x")', 'host.bus.call("system/voice", "say", {})', 'host.files.list("")', 'host.tasks.start({})', 'host.schedule.add({})']) {
        const checked = checkPackage({ ...SNAKE, backend: `${SNAKE.backend}\nexport async function more() { return ${call}; }` });
        expect(!checked.ok, `${call} passed`);
    }
    const quiet = checkPackage({ ...SNAKE, backend: `${SNAKE.backend}\n// host.bus.call в комментарии — не вызов\nconst s = "host.http.fetch";` });
    expect(quiet.ok, `words in comments and strings are not calls: ${quiet.problems.join('; ')}`);
    const hidden = checkPackage({ ...SNAKE, backend: `${SNAKE.backend}\nexport const x = () => \`\${host.bus.list()}\`;` });
    expect(!hidden.ok, 'a call inside a template string is a call');
});

check('doors past the host stop a package: fetch, sockets, eval, import()', () => {
    for (const door of ['fetch("https://x")', 'new WebSocket("wss://x")', 'eval("1")', 'import("./x.js")', 'new Function("return 1")']) {
        expect(!checkPackage({ ...SNAKE, backend: `${SNAKE.backend}\nexport function x() { return ${door}; }` }).ok, `backend: ${door}`);
        expect(!checkPackage({ ...SNAKE, panel: `${SNAKE.panel}\nfunction x() { return ${door}; }` }).ok, `panel: ${door}`);
    }
});

check('the panel may take from the SDK only what the web host gives', () => {
    const shared = SNAKE.panel.replace('useApi, usePanelState, useT }', 'useApi, usePanelState, useT, share }');
    expect(shared !== SNAKE.panel, 'the import line moved');
    const checked = checkPackage({ ...SNAKE, panel: shared });
    expect(!checked.ok && checked.problems.some((p) => p.includes('«share»')), checked.problems.join('; '));
    const other = checkPackage({ ...SNAKE, panel: `import x from "https://cdn.example/x.js";\n${SNAKE.panel}` });
    expect(!other.ok, 'a module from elsewhere');
});

check('a call the manifest names must be exported', () => {
    const checked = checkPackage({ ...SNAKE, backend: SNAKE.backend.replace('export function score', 'function score') });
    expect(!checked.ok && checked.problems.some((p) => p.includes('«score»')), checked.problems.join('; '));
});

check('files beyond the four do not go', () => {
    const checked = checkPackage({ ...SNAKE, names: [...SNAKE.names, 'examples.json'] });
    expect(!checked.ok, 'examples.json');
});

check('the code reader: comments, strings, regexps, imports, exports', () => {
    const code = codeOnly('const a = "fetch("; // fetch(\nconst r = /["\']/g; /* host.bus */ host.kv.get(`${host.log("x")}`);');
    expect(!code.includes('fetch') && !code.includes('host.bus'), code);
    expect(code.includes('host.kv.get') && code.includes('host.log'), code);
    const imports = importsOf('import { h } from "preact";\nimport { a, b as c } from "@duowave/panel-sdk";\nimport x from "y";');
    expect(JSON.stringify([...imports]) === JSON.stringify([['preact', ['h']], ['@duowave/panel-sdk', ['a', 'b']], ['y', ['default']]]), JSON.stringify([...imports]));
    expect(exportsOf('export function a() {}\nexport async function b() {}\nexport const c = 1;').join() === 'a,b,c', 'exports');
    expect(mainSection(JSON.parse(SNAKE.manifest) as Record<string, unknown>) === 'neon-snake', 'the main section');
});

// ─── The backend in the browser: the same functions on `host` ───────────────────────────────────────────────────────

/** Модуль бэкенда с `host` и `console` этого вызова (глобальные — как в воркере). */
async function loadBackend(source: string, setup: Parameters<typeof makeHost>[0]) {
    const made = makeHost(setup);
    (globalThis as Record<string, unknown>).host = made.host;
    const dir = mkdtempSync(join(tmpdir(), 'web-host-'));
    const file = join(dir, 'backend.mjs');
    writeFileSync(file, source);
    try {
        const module = (await import(pathToFileURL(file).href)) as Record<string, unknown>;
        return { module, made };
    } finally {
        rmSync(dir, { recursive: true, force: true });
    }
}

check('the snake keeps its best score in the store; the host hands out the writes', async () => {
    const { module, made } = await loadBackend(SNAKE.backend, { id: 'neon-snake', version: '1', locale: 'en', languages: ['ru', 'en'], settings: {}, kv: {} });
    const calls = ['best', 'score'];
    expect(JSON.stringify(await callBackend(module, calls, 'best', {})) === '{"best":0,"games":0,"last":0}', 'empty at first');
    expect(made.taken().length === 0, 'reading writes nothing');
    const scored = (await callBackend(module, calls, 'score', { points: 12 })) as Record<string, unknown>;
    expect(scored.best === 12 && scored.record === true, JSON.stringify(scored));
    const writes = made.taken();
    expect(writes.length === 1 && writes[0][0] === 'stats', JSON.stringify(writes));
    let refused = '';
    await callBackend(module, calls, 'other', {}).catch((error: Error) => (refused = error.message));
    expect(refused.includes('contributes.panel.calls'), refused);
});

check('the teacher speaks the page’s language, keeps the chosen tune and its own settings', async () => {
    const pkg = teacher();
    const manifest = JSON.parse(pkg.manifest) as Record<string, unknown>;
    const setup = { id: 'piano-teacher', version: '0.1.0', locale: 'en', languages: languagesOf(manifest), settings: defaultSettings(manifest), kv: {} };
    expect(setup.settings.names === true, 'the default of a setting');
    const { module, made } = await loadBackend(pkg.backend, setup);
    const calls = ['state', 'choose', 'add', 'remove', 'keyboard', 'done'];
    const state = (await callBackend(module, calls, 'state', {})) as { melodies: Array<{ id: string; title: string; level: string }>; current: string; names: boolean };
    expect(state.melodies.length === 8 && state.current === 'oda', JSON.stringify(state.current));
    expect(state.melodies[0].title === 'Ode to Joy' && state.melodies[0].level === 'easy', JSON.stringify(state.melodies[0]));
    expect(state.names === true, 'names');
    await callBackend(module, calls, 'choose', { id: 'eliza' });
    expect(JSON.stringify(made.taken()) === '[["current","eliza"]]', 'the choice is a write');
    let refused = '';
    await callBackend(module, calls, 'choose', { id: 'nope' }).catch((error: Error) => (refused = error.message));
    expect(refused === 'There is no such tune.', refused);
});

check('the same teacher in Russian', async () => {
    const pkg = teacher();
    const { module } = await loadBackend(pkg.backend, { id: 'piano-teacher', version: '0.1.0', locale: 'ru', languages: ['ru', 'en'], settings: {}, kv: {} });
    const state = (await callBackend(module, ['state'], 'state', {})) as { melodies: Array<{ title: string }> };
    expect(state.melodies[0].title === 'Ода к радости', state.melodies[0].title);
});

check('what the shelf has not answers in words', () => {
    const { host } = makeHost({ id: 'x', version: '1', locale: 'en', languages: ['en'], settings: {}, kv: {} });
    let said = '';
    try {
        (host.http as unknown as { fetch: () => void }).fetch();
    } catch (error) {
        said = (error as Error).message;
    }
    expect(said.startsWith('host.http.fetch:'), said);
});

check('host.i18n as the gateway’s: the order of languages and the plural forms', () => {
    const { host } = makeHost({ id: 'x', version: '1', locale: 'ru', languages: ['en'], settings: {}, kv: {} });
    const T = { ru: { n: { one: '{count} нота', few: '{count} ноты', many: '{count} нот' } }, en: { only: 'only' } };
    expect(host.i18n.t(T, 'n', { count: 21 }) === '21 нота', host.i18n.t(T, 'n', { count: 21 }));
    expect(host.i18n.t(T, 'n', { count: 12 }) === '12 нот', 'many');
    expect(host.i18n.t(T, 'only') === 'only', 'English when the language has none');
    expect(host.i18n.pick({ en: 'Hello', de: 'Hallo' }) === 'Hello', 'pick');
});

// ─── The frame is one: the gateway's and the web host's ─────────────────────────────────────────────────────────────

check('the gateway’s frame and the web host’s boot the same code', () => {
    const main = read('panel/src/frame/main.tsx');
    const web = read('panel/src/web-host/frame.tsx');
    expect(main.includes("from './boot'") && web.includes("from '../frame/boot'"), 'both through boot.tsx');
    expect(web.includes('readMidiFile: null'), 'no .mid on the site');
    expect(read('crates/duowave-apps/src/plugins/workshop/templates/midi/panel.js').includes('readMidiFile && h("button"'), 'the teacher hides the file button');
});

let failed = 0;
for (const [name, run] of checks) {
    try {
        await run();
        console.log(`✓ ${name}`);
    } catch (error) {
        failed += 1;
        console.error(`✗ ${name}\n    ${error instanceof Error ? error.message : String(error)}`);
    }
}
console.log(failed ? `${failed} of ${checks.length} failed` : `${checks.length} checks passed`);
process.exit(failed ? 1 : 0);
