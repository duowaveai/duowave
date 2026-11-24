#!/usr/bin/env node
// The models' prices built into the gateway (gateway/src/router/prices.json; docs/plans/router-and-works.md, block A,
// 2): a snapshot of models.dev, made before a release. Only the providers of «Мозг»'s presets
// (gateway/src/dialog/brain/presets.rs) and only what a price needs: dollars per million tokens of the input, the
// output, the cache read and the cache written, and the context's limit. The gateway never asks models.dev itself.
//
//   node scripts/dev/prices.mjs                  # from https://models.dev/api.json
//   node scripts/dev/prices.mjs --from api.json  # from a copy of it on disk
//
// A preset the table below does not know stops the script: decide where its prices come from first.
import { readFileSync, writeFileSync } from 'node:fs';
import path from 'node:path';

const ROOT = path.resolve(path.dirname(new URL(import.meta.url).pathname), '..', '..');
const PRESETS = path.join(ROOT, 'crates/duowave-brain/src/dialog/brain/presets.rs');
const OUT = path.join(ROOT, 'crates/duowave-brain/src/router/prices.json');
const SOURCE = 'https://models.dev/api.json';

// A preset of «Мозг» → its provider in models.dev; null — none to take: at home (0 by itself), priced as another
// preset (ChatGPT's plan as OpenAI's key, src/router/pool.rs), not in models.dev, or an address of one's own.
const PROVIDERS = {
    claude: 'anthropic',
    openai: 'openai',
    chatgpt: null,
    openrouter: 'openrouter',
    ollama: null,
    lmstudio: null,
    yandexgpt: null,
    gemini: 'google',
    deepseek: 'deepseek',
    groq: 'groq',
    mistral: 'mistral',
    'custom-openai': null,
    'custom-anthropic': null,
};

// Models that write no words for a conversation: embeddings, pictures, speech, moderation.
const NOT_CHAT = /embed|moderation|whisper|transcribe|tts|dall-e|image|realtime|audio/i;

function presetIds() {
    const source = readFileSync(PRESETS, 'utf8');
    const list = source.slice(source.indexOf('pub const PROVIDER_PRESETS'));
    return [...list.matchAll(/^\s+id: "([^"]+)",$/gm)].map((match) => match[1]);
}

async function load() {
    const from = process.argv.indexOf('--from');
    if (from > 0) {
        return JSON.parse(readFileSync(process.argv[from + 1], 'utf8'));
    }
    const response = await fetch(SOURCE, { headers: { accept: 'application/json' } });
    if (!response.ok) {
        throw new Error(`${SOURCE}: HTTP ${response.status}`);
    }
    return response.json();
}

/** A price as it is kept: a number, or null when there is none. */
function number(value) {
    return typeof value === 'number' && Number.isFinite(value) && value >= 0 ? value : null;
}

function line(model) {
    const cost = model.cost ?? {};
    const output = model.modalities?.output ?? ['text'];
    if (number(cost.input) === null || number(cost.output) === null || !output.includes('text')) {
        return null;
    }
    if (NOT_CHAT.test(model.id ?? '')) {
        return null;
    }
    const context = Number.isInteger(model.limit?.context) && model.limit.context > 0 ? model.limit.context : null;
    return [number(cost.input), number(cost.output), number(cost.cache_read), number(cost.cache_write), context];
}

const ids = presetIds();
const unknown = ids.filter((id) => !(id in PROVIDERS));
if (unknown.length > 0) {
    console.error(`presets.rs has presets this script does not know: ${unknown.join(', ')} — add them to PROVIDERS`);
    process.exit(1);
}
const gone = Object.keys(PROVIDERS).filter((id) => !ids.includes(id));
if (gone.length > 0) {
    console.error(`PROVIDERS names presets presets.rs no longer has: ${gone.join(', ')}`);
    process.exit(1);
}

const all = await load();
const providers = {};
const counts = [];
for (const id of ids) {
    const from = PROVIDERS[id];
    if (from === null) {
        continue;
    }
    const models = all[from]?.models;
    if (!models) {
        console.error(`models.dev has no provider ${from} (for the preset ${id})`);
        process.exit(1);
    }
    const kept = {};
    for (const name of Object.keys(models).sort()) {
        const priced = line({ id: name, ...models[name] });
        if (priced) {
            kept[name] = priced;
        }
    }
    providers[id] = kept;
    counts.push(`${id} ${Object.keys(kept).length}`);
}

// One model a line: a new snapshot reads as a diff of prices.
const day = new Date().toISOString().slice(0, 10);
const lines = [`{"schema":1,"from":"models.dev","at":"${day}","providers":{`];
const presets = Object.keys(providers);
presets.forEach((preset, p) => {
    lines.push(`${JSON.stringify(preset)}:{`);
    const names = Object.keys(providers[preset]);
    names.forEach((name, n) => {
        const comma = n + 1 < names.length ? ',' : '';
        lines.push(`${JSON.stringify(name)}:${JSON.stringify(providers[preset][name])}${comma}`);
    });
    lines.push(p + 1 < presets.length ? '},' : '}');
});
lines.push('}}');
const text = `${lines.join('\n')}\n`;
writeFileSync(OUT, text);
console.log(`${path.relative(ROOT, OUT)}: ${Buffer.byteLength(text)} bytes; ${counts.join(', ')}`);
