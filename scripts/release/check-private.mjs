#!/usr/bin/env node
// Nothing private in what goes public: keys and tokens, home folders, and — from a list kept outside git — the terms
// of one household (its addresses, its town, its names). CI runs the general checks; the snapshot runs both.
//
//   node scripts/release/check-private.mjs               # the tracked files, as the snapshot would take them
//   node scripts/release/check-private.mjs --dir <tree>  # a built snapshot
//
// The household's list: .private-terms at the repository root (gitignored) or PRIVATE_TERMS_FILE — one term a line,
// matched case-insensitively; # starts a comment.
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
const SKIP = /\.(png|jpe?g|gif|webp|ico|icns|woff2?|ttf|onnx|wav|ogg|opus|mp3|pt|safetensors|gz|zip|tgz)$|(^|\/)pnpm-lock\.yaml$/i;

function excluded() {
    return readFileSync(path.join(ROOT, 'scripts/release/exclude.txt'), 'utf8')
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

function privateTerms() {
    const file = process.env.PRIVATE_TERMS_FILE || path.join(ROOT, '.private-terms');
    if (!existsSync(file)) return [];
    return readFileSync(file, 'utf8')
        .split('\n')
        .map((line) => line.replace(/(^|\s)#.*/, '').trim())
        .filter(Boolean);
}

const dirIndex = process.argv.indexOf('--dir');
const files = dirIndex > 0 ? treeFiles(path.resolve(process.argv[dirIndex + 1])) : trackedFiles();
const terms = privateTerms();
const findings = [];
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
    const lines = text.split('\n');
    lines.forEach((line, index) => {
        for (const [what, pattern] of GENERAL) if (pattern.test(line)) findings.push(`${name}:${index + 1}: ${what}`);
        const lower = line.toLowerCase();
        for (const term of terms) if (lower.includes(term.toLowerCase())) findings.push(`${name}:${index + 1}: a private term (${term.length} chars)`);
    });
}

console.log(`checked ${files.length} files${terms.length ? `, ${terms.length} private terms` : ' (no .private-terms here: general checks only)'}`);
if (findings.length) {
    console.error(findings.join('\n'));
    console.error(`${findings.length} finding(s): nothing private goes public`);
    process.exit(1);
}
console.log('nothing private found');
