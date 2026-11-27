#!/usr/bin/env node
// metadata-worker.mjs without Cloudflare: its one path, the document it serves, its headers, its 404s and 405s. Node's
// own Request and Response stand for the runtime's.
//
//   node scripts/release/cloudflare/metadata-worker-check.mjs
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import worker, { CLIENT_ID, DOCUMENT, PATH } from './metadata-worker.mjs';

const URL_SERVED = 'https://metadata.duowave.app/oauth/client-metadata.json';
const get = (path, method = 'GET') => worker.fetch(new Request(`https://metadata.duowave.app${path}`, { method }));

let failures = 0;
async function check(what, body) {
    try {
        await body();
        console.log(`ok ${what}`);
    } catch (error) {
        failures += 1;
        console.log(`FAIL ${what}: ${error.message}`);
    }
}

await check('the document, as JSON, cached, no redirect', async () => {
    const answer = await get(PATH);
    assert.equal(answer.status, 200);
    assert.equal(answer.redirected, false);
    assert.equal(answer.headers.get('location'), null);
    assert.equal(answer.headers.get('content-type'), 'application/json');
    assert.equal(answer.headers.get('cache-control'), 'public, max-age=3600');
    assert.equal(await answer.text(), DOCUMENT);
});
await check('its client_id is the address it is served at', async () => {
    assert.equal(CLIENT_ID, URL_SERVED);
    assert.equal(JSON.parse(await (await get(PATH)).text()).client_id, URL_SERVED);
});
await check('the fields an authorization server reads', async () => {
    assert.deepEqual(JSON.parse(DOCUMENT), {
        client_id: URL_SERVED,
        client_name: 'DuoWave',
        client_uri: 'https://duowave.ai',
        logo_uri: 'https://duowave.ai/icon-512.png',
        redirect_uris: ['http://localhost:7780/api/v1/mcp/oauth/callback'],
        grant_types: ['authorization_code', 'refresh_token'],
        response_types: ['code'],
        token_endpoint_auth_method: 'none',
        application_type: 'native',
    });
});
await check("the gateway's constant names this address", async () => {
    const source = readFileSync(new URL('../../../crates/duowave-apps/src/mcp/mcp_oauth.rs', import.meta.url), 'utf8');
    const constant = source.match(/pub const CLIENT_METADATA_URL: &str = "([^"]+)";/);
    assert.ok(constant, 'CLIENT_METADATA_URL not found');
    assert.equal(constant[1], URL_SERVED, `the gateway sends ${constant[1]}`);
});
await check('a query string does not matter', async () => {
    assert.equal((await get(`${PATH}?x=1`)).status, 200);
});
await check('HEAD answers with the headers and no body', async () => {
    const head = await get(PATH, 'HEAD');
    assert.equal(head.status, 200);
    assert.equal(head.headers.get('content-type'), 'application/json');
    assert.equal(await head.text(), '');
});
await check('everything else is 404', async () => {
    for (const path of ['/', '/oauth', '/oauth/', '/oauth/client-metadata.json/', '/client-metadata.json', '/oauth/client-metadata', '/OAUTH/client-metadata.json', '/.well-known/oauth-client']) {
        assert.equal((await get(path)).status, 404, path);
    }
});
await check('POST, PUT and DELETE are 405', async () => {
    for (const method of ['POST', 'PUT', 'DELETE']) {
        const answer = await get(PATH, method);
        assert.equal(answer.status, 405, method);
        assert.equal(answer.headers.get('allow'), 'GET, HEAD');
    }
});

console.log(`${failures} failed`);
process.exit(failures);
