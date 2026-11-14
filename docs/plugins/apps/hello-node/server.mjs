// «Привет на Node» — самое маленькое приложение `runtime: node` (docs/plugins/AUTHORING.md, «Приложение на Node»).
// Шлюз запускает его своим Node: `node server.mjs` с PORT, DUOWAVE_URL, DUOWAVE_APP_TOKEN, DUOWAVE_APP_ID и
// DUOWAVE_APP_DATA в окружении. Функции из манифеста шлюз зовёт как POST /call/<handler> с тем же токеном; здоровье
// спрашивает GET /health. Host API — тот же, что у любого приложения: /api/v1/apps/host/… с токеном.
import http from 'node:http';

const { PORT, DUOWAVE_URL, DUOWAVE_APP_TOKEN } = process.env;

/** Просьба к шлюзу от имени приложения. */
function host(method, path, body) {
    return fetch(`${DUOWAVE_URL}${path}`, {
        method,
        headers: { authorization: `Bearer ${DUOWAVE_APP_TOKEN}`, 'content-type': 'application/json' },
        body: body === undefined ? undefined : JSON.stringify(body),
    });
}

/** Письмо на шине от имени приложения: адрес, умение, данные — как host.bus.call у плагина. */
async function letter(to, handle, input) {
    const answer = await host('POST', '/api/v1/apps/host/call/bus.call', { to, handle, input });
    const body = await answer.json().catch(() => null);
    if (!answer.ok) throw new Error(body?.error ?? `HTTP ${answer.status}`);
    return body;
}

const calls = {
    /** Здоровается и запоминает имя в своём хранилище (kv). */
    async hello(input) {
        const name = typeof input?.name === 'string' && input.name.trim() ? input.name.trim() : 'мир';
        const stored = await host('PUT', '/api/v1/apps/host/kv/last', { name });
        return {
            text: `Привет, ${name}! Это Node ${process.version}.`,
            node: process.version,
            pid: process.pid,
            stored: stored.status === 204,
            // Своя папка на шине (files/app/<id>): что программа кладёт сюда, окно играет через useMedia.
            files: process.env.DUOWAVE_APP_FILES ?? null,
        };
    },
    /**
     * Абзац моделью шлюза и он же голосом компьютера в свою папку (system/brain → write, system/voice → synth): файл
     * voice/story.wav окно играет через useMedia. Голос по умолчанию — Piper «Денис» (CC0, можно публиковать).
     */
    async story(input) {
        const about = typeof input?.about === 'string' && input.about.trim() ? input.about.trim() : 'море';
        const written = await letter('system/brain', 'write', { prompt: `Напиши два коротких предложения о том, что такое ${about}.`, model: 'fast' });
        const voiced = await letter('system/voice', 'synth', { text: written.text, to: 'voice/story.wav' });
        return { text: written.text, model: written.model, file: voiced.file, seconds: voiced.seconds, voice: voiced.id, publish: voiced.publish };
    },
    /** Падает чуть позже ответа: надзор шлюза запустит программу снова. */
    crash() {
        setTimeout(() => process.exit(3), 50);
        return { crashing: true };
    },
};

http.createServer(async (request, response) => {
    if (request.method === 'GET' && request.url === '/health') {
        response.writeHead(200).end('ok');
        return;
    }
    const match = request.method === 'POST' ? request.url?.match(/^\/call\/([A-Za-z0-9_]+)$/) : null;
    const call = match && Object.hasOwn(calls, match[1]) ? calls[match[1]] : null;
    if (!call) {
        response.writeHead(404).end();
        return;
    }
    if (request.headers.authorization !== `Bearer ${DUOWAVE_APP_TOKEN}`) {
        response.writeHead(401).end();
        return;
    }
    let body = '';
    for await (const chunk of request) body += chunk;
    try {
        const result = await call(body ? JSON.parse(body) : null);
        response.writeHead(200, { 'content-type': 'application/json' }).end(JSON.stringify(result));
    } catch (error) {
        response.writeHead(500).end(String(error?.message ?? error));
    }
}).listen(Number(PORT), '127.0.0.1');
