#!/usr/bin/env node
// Dockerfile.prod builds the gateway from source, and everything the program carries inside from outside its own
// folder has to reach that build: let into the context by .dockerignore and copied into the gateway stage, where cargo
// runs. In 0.2.0 it did not (the station's scripts were renamed, the Dockerfile still named the old one), and nothing
// noticed: CI builds no image. This reads the code instead — include_str!/include_bytes! with a path and rust-embed's
// #[folder], resolved as the compiler resolves them — and checks each such path, and each file the Dockerfile copies
// from the context, against the two files. Node's built-in modules only.
//
//   node scripts/check-docker-context.mjs
import { existsSync, readdirSync, readFileSync, statSync } from 'node:fs';
import path from 'node:path';

const ROOT = path.resolve(path.dirname(new URL(import.meta.url).pathname), '..');
const GATEWAY = path.join(ROOT, 'gateway');
const posix = (full) => path.relative(ROOT, full).split(path.sep).join('/');

/** What the gateway's code embeds from outside gateway/: repository path → where the code names it. */
function embedded() {
    const found = new Map();
    const note = (full, where) => {
        const file = posix(full);
        if (file !== 'gateway' && !file.startsWith('gateway/') && !found.has(file)) found.set(file, where);
    };
    const read = (source) => {
        const text = readFileSync(source, 'utf8');
        const at = (index) => `${posix(source)}:${text.slice(0, index).split('\n').length}`;
        for (const match of text.matchAll(/include_(?:str|bytes)!\(\s*"([^"]+)"\s*\)/g)) {
            note(path.resolve(path.dirname(source), match[1]), at(match.index));
        }
        // rust-embed's folders are relative to the crate (CARGO_MANIFEST_DIR).
        for (const match of text.matchAll(/#\[folder\s*=\s*"([^"]+)"\s*\]/g)) {
            note(path.resolve(GATEWAY, match[1]), at(match.index));
        }
    };
    const walk = (folder) => {
        for (const name of readdirSync(folder)) {
            const full = path.join(folder, name);
            if (statSync(full).isDirectory()) walk(full);
            else if (name.endsWith('.rs')) read(full);
        }
    };
    walk(path.join(GATEWAY, 'src'));
    return found;
}

/** .dockerignore's lines as patterns, in order: `*`, `**`, `?` as Docker reads them; `!` lets back in. */
function ignoreRules() {
    return readFileSync(path.join(ROOT, '.dockerignore'), 'utf8')
        .split('\n')
        .map((line) => line.trim())
        .filter((line) => line && !line.startsWith('#'))
        .map((line) => {
            const negated = line.startsWith('!');
            const glob = (negated ? line.slice(1) : line).trim().replace(/^\/+|\/+$/g, '');
            let source = '';
            for (let index = 0; index < glob.length; index += 1) {
                const char = glob[index];
                if (char === '*' && glob[index + 1] === '*') {
                    index += 1;
                    if (glob[index + 1] === '/') {
                        index += 1;
                        source += '(?:.*/)?';
                    } else source += '.*';
                } else if (char === '*') source += '[^/]*';
                else if (char === '?') source += '[^/]';
                else source += char.replace(/[.+^${}()|[\]\\]/g, '\\$&');
            }
            return { line, pattern: new RegExp(`^${source}$`), negated };
        });
}

/** Whether the context has the path: the last line that matches it, or a folder above it, decides. */
function letIn(file, rules) {
    const parts = file.split('/');
    let inside = true;
    for (const rule of rules) {
        for (let depth = 1; depth <= parts.length; depth += 1) {
            if (rule.pattern.test(parts.slice(0, depth).join('/'))) {
                inside = rule.negated;
                break;
            }
        }
    }
    return inside;
}

/** Dockerfile.prod's COPY lines by stage, each with where its sources land. */
function copies() {
    const text = readFileSync(path.join(ROOT, 'Dockerfile.prod'), 'utf8').replace(/\\\n/g, ' ');
    const out = [];
    let stage = '';
    let workdir = '/';
    for (const raw of text.split('\n')) {
        const line = raw.trim();
        if (!line || line.startsWith('#')) continue;
        const [instruction, ...words] = line.split(/\s+/);
        switch (instruction.toUpperCase()) {
            case 'FROM':
                stage = /\sAS\s+(\S+)\s*$/i.exec(line)?.[1] ?? '';
                workdir = '/';
                break;
            case 'WORKDIR':
                workdir = path.posix.resolve(workdir, words[0]);
                break;
            case 'COPY': {
                const args = words.filter((word) => !word.startsWith('--'));
                const from = words.find((word) => word.startsWith('--from='))?.slice('--from='.length);
                const target = args.at(-1);
                out.push({
                    stage,
                    line,
                    from,
                    sources: args.slice(0, -1),
                    dest: path.posix.resolve(workdir, target),
                    intoFolder: target.endsWith('/') || args.length > 2,
                    workdir,
                });
                break;
            }
        }
    }
    return out;
}

/** Where a repository path lands through one COPY, or null when that COPY does not carry it. */
function landing(file, copy) {
    for (const source of copy.sources) {
        // Another stage's paths are its own; ours build under the same /src (the panel's dist).
        const from = copy.from ? path.posix.relative('/src', source) : source.replace(/^\.\//, '').replace(/\/+$/, '');
        if (file === from) {
            const folder = copy.from || (existsSync(path.join(ROOT, from)) && statSync(path.join(ROOT, from)).isDirectory());
            return folder || !copy.intoFolder ? copy.dest : path.posix.join(copy.dest, path.posix.basename(from));
        }
        if (file.startsWith(`${from}/`)) return path.posix.join(copy.dest, file.slice(from.length + 1));
    }
    return null;
}

const rules = ignoreRules();
const all = copies();
const problems = [];

// Each file the Dockerfile copies from the context exists and is let in (a build argument in the path is skipped).
for (const copy of all.filter((copy) => !copy.from)) {
    for (const source of copy.sources.filter((source) => !source.includes('$'))) {
        const file = source.replace(/^\.\//, '').replace(/\/+$/, '');
        if (!existsSync(path.join(ROOT, file))) problems.push(`${copy.stage}: «${copy.line}» copies ${file}, which is not in the repository`);
        else if (!letIn(file, rules)) problems.push(`${copy.stage}: «${copy.line}» copies ${file}, which .dockerignore leaves out`);
    }
}

// Each file the program carries inside lands where the code expects it in the gateway stage.
const gateway = all.filter((copy) => copy.stage === 'gateway');
if (gateway.length === 0) problems.push('Dockerfile.prod has no stage named gateway');
for (const [file, where] of embedded()) {
    if (!existsSync(path.join(ROOT, file))) {
        // The panel's dist is built (in the image by its own stage); anything else missing is the code's own error.
        if (file !== 'panel/dist') problems.push(`${where} embeds ${file}, which is not in the repository`);
    }
    const expected = path.posix.join(gateway[0]?.workdir ?? '/src', file);
    const copy = gateway.find((candidate) => landing(file, candidate) === expected);
    if (!copy) {
        problems.push(`${where} embeds ${file}: no COPY in the gateway stage puts it at ${expected}`);
        continue;
    }
    if (!copy.from && !letIn(file, rules)) problems.push(`${where} embeds ${file}: .dockerignore leaves it out of the context`);
}

if (problems.length) {
    console.error(`Dockerfile.prod cannot build the gateway from source:\n  ${problems.join('\n  ')}`);
    process.exit(1);
}
console.log(`ok: the gateway stage has all ${embedded().size} files the program carries from outside gateway/`);
