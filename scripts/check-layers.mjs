#!/usr/bin/env node
// The gateway's floors (docs/ARCHITECTURE.md, «Этажи»; docs/plans/modular-gateway.md): every top module of
// gateway/src and of the workspace's crates (crates/*/src) stands on a floor in gateway/layers.json, and a module may use its own floor and the floors below, never
// one above. The floors are where the crates of the workspace are cut; inside one crate the walls hold only on
// discipline, so this reads the code instead — each `crate::<module>` (or `duowave_<crate>::<module>`) outside comments
// and test code — and fails on a link up that is not already among the known exceptions. The exceptions are the work list: the list only shrinks,
// and an exception that is gone fails too, so that it is struck off and cannot come back unnoticed.
// Node's built-in modules only, no cargo: a second, not a build.
//
//   node scripts/check-layers.mjs            check
//   node scripts/check-layers.mjs --update   strike the exceptions that are gone off gateway/layers.json (never adds)
//   node scripts/check-layers.mjs --report   the exceptions floor by floor, with where each is used
import { existsSync, readdirSync, readFileSync, statSync, writeFileSync } from 'node:fs';
import path from 'node:path';

const ROOT = path.resolve(path.dirname(new URL(import.meta.url).pathname), '..');
// The source roots: the program's (gateway/src) and each crate's (crates/<name>/src); a module's name is unique among them.
const CRATES = path.join(ROOT, 'crates');
// The floor «Только компьютер» (crates/duowave-pc) also holds the computer's parts of modules of the floors below,
// under their names (`models` there adds the model shop to «Голос»'s `models`): its files stand on its own floor
// whatever their module is called. Read last, so that a name it shares stays the lower crate's module.
const PC = path.join(CRATES, 'duowave-pc', 'src');
const ROOTS = [
    path.join(ROOT, 'gateway', 'src'),
    ...(existsSync(CRATES) ? readdirSync(CRATES).sort().map((name) => path.join(CRATES, name, 'src')) : [])
        .filter(existsSync)
        .sort((a, b) => (a === PC) - (b === PC)),
];
const TABLE = path.join(ROOT, 'gateway', 'layers.json');
const posix = (full) => path.relative(ROOT, full).split(path.sep).join('/');
const args = new Set(process.argv.slice(2));

/** Every .rs file under a folder. */
function sources(folder) {
    const found = [];
    for (const name of readdirSync(folder).sort()) {
        const full = path.join(folder, name);
        if (statSync(full).isDirectory()) found.push(...sources(full));
        else if (name.endsWith('.rs')) found.push(full);
    }
    return found;
}

// The crates' top modules: their folders and their top files, without a crate's root (lib.rs) and the programs beside
// the library (main.rs, bin/), which reach it as `duowave::`, not `crate::`.
const PROGRAMS = new Set(['lib.rs', 'main.rs', 'bin']);
const rootOf = new Map();
// The modules of the floors below that the computer's floor adds to (see PC): `pc:<name>` for its own part.
const merged = new Set();
for (const src of ROOTS) {
    for (const name of readdirSync(src)) {
        if (PROGRAMS.has(name)) continue;
        if (!statSync(path.join(src, name)).isDirectory() && !name.endsWith('.rs')) continue;
        const module = name.replace(/\.rs$/, '');
        if (src === PC && rootOf.has(module)) {
            merged.add(module);
            continue;
        }
        rootOf.set(module, src);
    }
}
const modules = [...rootOf.keys()].sort();

// `crate::<name>` that is not a module: an item of lib.rs (`pub use platform::http::ApiError` is platform's; a const or
// fn of its own is the root's, `lib`) or a #[macro_export] macro, which lives at the crate root but is its module's.
// A crate root's `pub use` of another crate (`pub use duowave_base::…`) only passes its names on: the module or item
// is the other crate's.
const owner = new Map();
for (const src of ROOTS) {
    const lib = existsSync(path.join(src, 'lib.rs')) ? readFileSync(path.join(src, 'lib.rs'), 'utf8') : '';
    for (const match of lib.matchAll(/pub use (\w+)::(?:\w+::)*(\w+);/g)) {
        if (!match[1].startsWith('duowave')) owner.set(match[2], match[1]);
    }
    for (const match of lib.matchAll(/pub (?:const|fn|static) (\w+)/g)) owner.set(match[1], 'lib');
}
const files = ROOTS.flatMap((src) => sources(src));
const topOf = (full) => {
    const src = ROOTS.find((root) => full.startsWith(root + path.sep));
    const module = path.relative(src, full).split(path.sep)[0].replace(/\.rs$/, '');
    return src === PC && merged.has(module) ? `pc:${module}` : module;
};
for (const full of files) {
    const text = readFileSync(full, 'utf8');
    for (const match of text.matchAll(/#\[macro_export\]\s*(?:#\[[^\]]*\]\s*)*macro_rules!\s*(\w+)/g)) {
        owner.set(match[1], topOf(full));
    }
}

/** Which lines of a file are test code: a `#[cfg(test)]` or `#[cfg(all(test, …))]` item to its closing brace at the
 * item's own indent (the code is rustfmt's), and files such a `mod name;` brings in — by its name, or by its
 * `#[path = "…"]`. `any(test, …)` builds without tests too: code. */
const testFiles = new Set();
const TEST_ONLY = /^#\[cfg\((?:test|all\(test\b.*)\)\]$/;
function testLines(full, lines) {
    const test = new Array(lines.length).fill(false);
    for (let i = 0; i < lines.length; i++) {
        if (!TEST_ONLY.test(lines[i].trim())) continue;
        const indent = lines[i].slice(0, lines[i].length - lines[i].trimStart().length);
        let j = i + 1;
        let named = null;
        while (j < lines.length && lines[j].trim().startsWith('#[')) {
            named = lines[j].trim().match(/^#\[path\s*=\s*"([^"]+)"\]$/)?.[1] ?? named;
            j++;
        }
        let end = j;
        if (j < lines.length && lines[j].trimEnd().endsWith(';')) {
            const external = lines[j].match(/^\s*(?:pub(?:\(crate\))? )?mod (\w+);/);
            if (external && named) {
                // A `mod` outside an inline module: its path from the directory of the file that names it.
                testFiles.add(path.join(path.dirname(full), named));
            } else if (external) {
                const base = ['mod.rs', 'lib.rs'].includes(path.basename(full))
                    ? path.dirname(full)
                    : full.replace(/\.rs$/, '');
                testFiles.add(path.join(base, `${external[1]}.rs`));
                testFiles.add(path.join(base, external[1]) + path.sep);
            }
        } else {
            while (end < lines.length && lines[end] !== `${indent}}`) end++;
        }
        for (let k = i; k <= Math.min(end, lines.length - 1); k++) test[k] = true;
        i = end;
    }
    return test;
}

// Read every file before looking for links: a test module's own file (`#[cfg(test)] mod tests;`) is known only once
// its parent is read.
const read = files.map((full) => {
    const lines = readFileSync(full, 'utf8').split('\n');
    return { full, lines, test: testLines(full, lines) };
});
const isTestFile = (full) => [...testFiles].some((test) => full === test || full.startsWith(test));

/** module → module → where it is used (code only). */
const links = new Map();
const unknown = [];
for (const { full, lines, test } of read) {
    const from = topOf(full);
    if (PROGRAMS.has(from) || PROGRAMS.has(`${from}.rs`)) continue;
    if (isTestFile(full)) continue;
    lines.forEach((line, index) => {
        if (test[index]) return;
        // Comments out: a whole-line one always, a trailing one when no string on the line could hold a `//`.
        const code = line.includes('"') ? line.replace(/^\s*\/\/.*$/, '') : line.split('//')[0];
        for (const match of code.matchAll(/(?<![\w$])(?:crate|duowave_\w+)::(\w+)/g)) {
            const name = match[1];
            const to = modules.includes(name) ? name : owner.get(name);
            const where = `${posix(full)}:${index + 1}`;
            if (!to) {
                unknown.push(`crate::${name} at ${where}`);
                continue;
            }
            if (to === from) continue;
            if (!links.has(from)) links.set(from, new Map());
            const uses = links.get(from);
            if (!uses.has(to)) uses.set(to, []);
            uses.get(to).push(where);
        }
    });
}

const table = JSON.parse(readFileSync(TABLE, 'utf8'));
const floorOf = new Map();
const problems = [];
table.floors.forEach((floor, level) => {
    for (const module of floor.modules) {
        if (floorOf.has(module)) problems.push(`${module} stands on two floors in gateway/layers.json`);
        floorOf.set(module, { level, ...floor });
    }
});
// The computer's parts of lower modules stand on the computer's floor.
const pcFloor = table.floors.findIndex((floor) => floor.name === 'pc');
for (const module of merged) floorOf.set(`pc:${module}`, { level: pcFloor, ...table.floors[pcFloor] });
for (const module of [...modules, 'lib']) {
    if (!floorOf.has(module)) {
        problems.push(`a new module, ${module}: put it on a floor in gateway/layers.json (docs/ARCHITECTURE.md, «Этажи»)`);
    }
}
for (const module of floorOf.keys()) {
    const part = module.startsWith('pc:') && merged.has(module.slice(3));
    if (module !== 'lib' && !part && !modules.includes(module)) {
        problems.push(`${module} is in gateway/layers.json, but no crate has such a module any more`);
    }
}
for (const what of unknown) problems.push(`${what}: neither a module, nor an item of lib.rs, nor an exported macro`);
if (problems.length) {
    console.error(`gateway floors:\n  ${problems.join('\n  ')}`);
    process.exit(1);
}

const name = (module) => `${module} «${floorOf.get(module).title}»`;
const known = new Set(table.exceptions);
const up = [];
let count = 0;
for (const [from, uses] of links) {
    for (const [to, where] of uses) {
        count++;
        if (floorOf.get(to).level > floorOf.get(from).level) up.push({ key: `${from} -> ${to}`, from, to, where });
    }
}
up.sort((a, b) => a.key.localeCompare(b.key));
const current = new Set(up.map((link) => link.key));
const fresh = up.filter((link) => !known.has(link.key));
const gone = table.exceptions.filter((key) => !current.has(key));

if (args.has('--report')) {
    for (const [level, floor] of table.floors.entries()) {
        const mine = up.filter((link) => floorOf.get(link.from).level === level);
        if (!mine.length) continue;
        console.log(`\n${floor.title} (${floor.name}) — ${mine.length} up:`);
        for (const link of mine) {
            console.log(`  ${link.key.padEnd(28)} → «${floorOf.get(link.to).title}», ${link.where.length}× ${link.where[0]}`);
        }
    }
    console.log('');
}

if (args.has('--update')) {
    if (gone.length) {
        const kept = table.exceptions.filter((key) => current.has(key));
        writeFileSync(TABLE, format({ ...table, exceptions: kept }));
        console.log(`struck off ${gone.length}: ${gone.join(', ')}`);
    }
    gone.length = 0;
}

if (fresh.length) {
    console.error('gateway floors: a module reaches a floor above its own:');
    for (const link of fresh) {
        const more = link.where.length > 3 ? `, … (${link.where.length})` : '';
        console.error(`  ${name(link.from)} → ${name(link.to)}: ${link.where.slice(0, 3).join(', ')}${more}`);
    }
    console.error(
        'Take what it needs down a floor (a type, a constant), or ask through a small trait in links/ that app.rs fills in.\n' +
            'Upwards on purpose: add the line to "exceptions" in gateway/layers.json, and say why in the commit.',
    );
}
if (gone.length) {
    console.error('gateway floors: exceptions no longer needed — strike them off gateway/layers.json (or run with --update):');
    for (const key of gone) console.error(`  ${key}`);
}
if (fresh.length || gone.length) process.exit(1);
console.log(
    `ok: ${modules.length} modules on ${table.floors.length} floors, ${count} links between them, ` +
        `${up.length} of them up a floor — the known exceptions in gateway/layers.json`,
);

/** The table as it is kept: a floor a line, an exception a line. */
function format(data) {
    const floors = data.floors.map(
        (floor) =>
            `    { "name": ${JSON.stringify(floor.name)}, "title": ${JSON.stringify(floor.title)}, ` +
            `"modules": [${floor.modules.map((module) => JSON.stringify(module)).join(', ')}] }`,
    );
    const exceptions = data.exceptions.map((key) => `    ${JSON.stringify(key)}`);
    return (
        '{\n' +
        `  "_": ${JSON.stringify(data._)},\n` +
        `  "floors": [\n${floors.join(',\n')}\n  ],\n` +
        `  "exceptions": [\n${exceptions.join(',\n')}\n  ]\n` +
        '}\n'
    );
}

