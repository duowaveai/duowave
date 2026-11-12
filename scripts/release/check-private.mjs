#!/usr/bin/env node
// Nothing private in what goes public: keys and tokens, home folders, and — from a list kept outside git — the terms
// of one household (its addresses, its town, its names). CI runs the general checks; the snapshot runs both; and a
// push from this computer runs both on what it sends (.githooks/pre-push, on with `pnpm hooks`).
//
//   node scripts/release/check-private.mjs                 # the tracked files, as the snapshot would take them
//   node scripts/release/check-private.mjs --dir <tree>    # a built snapshot, or any tree: every file in it
//   node scripts/release/check-private.mjs --commits <rev> # the lines added by every commit of <rev> no remote has yet
//
// The household's list: .private-terms at the repository root (gitignored) or PRIVATE_TERMS_FILE — one term a line,
// matched case-insensitively; # starts a comment. A term as it is matches anywhere («192.168.1.5» inside an address);
// `=term` only as a whole word; `term*` at a word's start with any ending — a name and its cases («Ив*»: Ива, Иве, Иву).
import { execFileSync } from 'node:child_process';
import { existsSync, readdirSync, readFileSync, statSync } from 'node:fs';
import path from 'node:path';

const ROOT = path.resolve(path.dirname(new URL(import.meta.url).pathname), '..', '..');

const GENERAL = [
    ['a private key', /-----BEGIN (?:RSA |EC |DSA |OPENSSH |ENCRYPTED )?PRIVATE KEY-----/],
    ['an Anthropic key', /sk-ant-[A-Za-z0-9_-]{20,}/],
    ['an OpenAI-style key', /\bsk-(?:proj-)?[A-Za-z0-9]{32,}/],
    ['a GitHub token', /\bgh[pousr]_[A-Za-z0-9]{30,}/],
    ['a Slack token', /\bxox[abprs]-[A-Za-z0-9-]{10,}/],
    ['an AWS key', /\bAKIA[0-9A-Z]{16}\b/],
    ['a Google key', /\bAIza[0-9A-Za-z_-]{35}\b/],
    ['a Telegram bot token', /\b\d{8,10}:AA[A-Za-z0-9_-]{33}\b/],
    // A home folder names its person; the usual placeholders and CI's are fine, and so is the home server's API
    // (/home/v1, gateway/src/home_server).
    ['a home folder', /(?:\/Users|\/home)\/(?!(?:node|runner|user|owner|pi|v1|<[^>]+>|\$USER|\$\{USER\})\/)[a-z][\w.-]*\//],
];

/**
 * No voice in the tree, ever: recordings, a voice imprint, model weights. A household's voice is its own; the folders
 * that hold voices (omnivoice-server/models, tts-server/models, data/voice-lab) stay empty in git. A synthetic test
 * fixture that has to be audio is named here.
 */
const VOICE = /\.(wav|ogg|opus|mp3|m4a|flac|pcm|pt|pth|ckpt|safetensors)$|(^|\/)voice_prompt\b/i;
const VOICE_ALLOWED = new Set([]);

/** Binary and generated files the checks would only misread. */
const SKIP = /\.(png|jpe?g|gif|webp|ico|icns|woff2?|ttf|onnx|wav|ogg|opus|mp3|mp4|pt|safetensors|gz|zip|tgz)$|(^|\/)pnpm-lock\.yaml$/i;

/**
 * The files that carry the whole table of time zones: a zone's id names a city (`Asia/…`), the household's town among
 * them, and that tells nothing of anyone. Only here are the ids passed over; anywhere else a zone is a place.
 */
const ZONE_TABLES = new Set(['packages/calendar/backend.js']);
const ZONE = /\b(?:Africa|America|Antarctica|Asia|Atlantic|Australia|Europe|Indian|Pacific)\/[A-Za-z_]+(?:\/[A-Za-z_]+)?/g;

// exclude-internal.txt is itself left out of the public tree: there the check runs without it.
function excluded() {
    return ['exclude.txt', 'exclude-internal.txt']
        .map((file) => path.join(ROOT, 'scripts/release', file))
        .filter((file) => existsSync(file))
        .map((file) => readFileSync(file, 'utf8'))
        .join('\n')
        .split('\n')
        .map((line) => line.replace(/#.*/, '').trim())
        .filter(Boolean);
}

function trackedFiles() {
    const skip = excluded();
    return execFileSync('git', ['ls-files', '-z'], { cwd: ROOT, encoding: 'utf8' })
        .split('\0')
        .filter(Boolean)
        .filter((file) => !skip.some((entry) => file === entry || file.startsWith(`${entry}/`)))
        .map((file) => [file, path.join(ROOT, file)]);
}

function treeFiles(dir) {
    const out = [];
    const walk = (folder) => {
        for (const name of readdirSync(folder)) {
            const full = path.join(folder, name);
            if (name === '.git' || name === 'node_modules') continue;
            if (statSync(full).isDirectory()) walk(full);
            else out.push([path.relative(dir, full), full]);
        }
    };
    walk(dir);
    return out;
}

/** The checkout that holds .git, where the gitignored list lives when this runs in a worktree of it. */
function mainCheckout() {
    try {
        const common = execFileSync('git', ['rev-parse', '--path-format=absolute', '--git-common-dir'], { cwd: ROOT, encoding: 'utf8' });
        return path.dirname(common.trim());
    } catch {
        return undefined;
    }
}

/** The list's terms, each a test of one line: `=term` a whole word, `term*` a word's start, else anywhere. */
function privateTerms() {
    const file = process.env.PRIVATE_TERMS_FILE || [ROOT, mainCheckout()].map((dir) => dir && path.join(dir, '.private-terms')).find((f) => f && existsSync(f));
    if (!file || !existsSync(file)) return [];
    return readFileSync(file, 'utf8')
        .split('\n')
        .map((line) => line.replace(/(^|\s)#.*/, '').trim())
        .filter(Boolean)
        .map((term) => {
            const whole = term.startsWith('=');
            const start = !whole && term.length > 1 && term.endsWith('*');
            const word = whole ? term.slice(1) : start ? term.slice(0, -1) : term;
            const escaped = word.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
            const pattern = whole
                ? `(?<![\\p{L}\\p{N}])${escaped}(?![\\p{L}\\p{N}])`
                : start
                  ? `(?<![\\p{L}\\p{N}])${escaped}`
                  : escaped;
            return { length: word.length, pattern: new RegExp(pattern, 'iu') };
        });
}

const terms = privateTerms();

/** What one line of `name` gives away, if anything. */
function inLine(name, line) {
    const found = [];
    for (const [what, pattern] of GENERAL) if (pattern.test(line)) found.push(what);
    const text = ZONE_TABLES.has(name) ? line.replace(ZONE, '') : line;
    for (const term of terms) if (term.pattern.test(text)) found.push(`a private term (${term.length} chars)`);
    return found;
}

function checkFiles(files, findings) {
    for (const [name, full] of files) {
        if (VOICE.test(name) && !VOICE_ALLOWED.has(name)) findings.push(`${name}: a recording, a voice or model weights — never in the tree`);
        if (SKIP.test(name)) continue;
        let text;
        try {
            text = readFileSync(full, 'utf8');
        } catch {
            continue;
        }
        if (text.includes('\0')) continue;
        text.split('\n').forEach((line, index) => {
            for (const what of inLine(name, line)) findings.push(`${name}:${index + 1}: ${what}`);
        });
    }
}

/**
 * The lines every commit of `rev` adds that no remote has yet: what a push would leave in the history for good, even
 * where a later commit takes it out of the tree. A merge counts against its first parent.
 */
function checkCommits(rev, findings) {
    const git = (args) => execFileSync('git', args, { cwd: ROOT, encoding: 'utf8', maxBuffer: 1 << 30 });
    const commits = git(['rev-list', rev, '--not', '--remotes']).split('\n').filter(Boolean);
    for (const sha of commits) {
        const parents = git(['rev-list', '--parents', '-n', '1', sha]).trim().split(' ').slice(1);
        const base = parents[0] ?? git(['hash-object', '-t', 'tree', '/dev/null']).trim();
        const patch = git(['diff', '--no-color', '--no-ext-diff', '-U0', base, sha]);
        let name = null;
        for (const line of patch.split('\n')) {
            if (line.startsWith('diff --git ')) {
                name = line.replace(/^diff --git a\/.* b\//, '');
                if (VOICE.test(name) && !VOICE_ALLOWED.has(name)) findings.push(`${sha.slice(0, 7)} ${name}: a recording, a voice or model weights`);
            } else if (line.startsWith('+++ ')) {
                if (line === '+++ /dev/null') name = null;
            } else if (line.startsWith('+') && name && !SKIP.test(name)) {
                for (const what of inLine(name, line.slice(1))) findings.push(`${sha.slice(0, 7)} ${name}: ${what}`);
            }
        }
    }
    return commits.length;
}

const arg = (flag) => {
    const index = process.argv.indexOf(flag);
    return index > 0 ? process.argv[index + 1] : undefined;
};
const findings = [];
const what = terms.length ? `, ${terms.length} private terms` : ' (no .private-terms here: general checks only)';
if (arg('--commits')) {
    const count = checkCommits(arg('--commits'), findings);
    console.log(`checked ${count} commit(s) no remote has${what}`);
} else {
    const files = arg('--dir') ? treeFiles(path.resolve(arg('--dir'))) : trackedFiles();
    checkFiles(files, findings);
    console.log(`checked ${files.length} files${what}`);
}
if (findings.length) {
    console.error(findings.join('\n'));
    console.error(`${findings.length} finding(s): nothing private goes public`);
    process.exit(1);
}
console.log('nothing private found');
