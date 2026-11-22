#!/usr/bin/env node
// get-worker.mjs without Cloudflare: its paths, its 404s and what it hands on, against a pretend catalog. Node's own
// Request, Response and fetch stand for the runtime's.
//
//   node scripts/release/cloudflare/get-worker-check.mjs
import assert from 'node:assert/strict';
import worker, { served } from './get-worker.mjs';

const asked = [];
const catalog = {
    '/station.sh': ['#!/bin/sh\nmain() { :; }\nmain "$@"\n', 'application/x-sh'],
    '/station/0.18.3/duowave-station-aarch64.tar.gz': [new Uint8Array(70_000).fill(7), 'application/gzip'],
};
globalThis.fetch = async (url, init) => {
    asked.push([String(url), init?.method]);
    const found = catalog[new URL(url).pathname];
    if (!found) return new Response('Not Found', { status: 404, headers: { 'content-type': 'text/html' } });
    return new Response(found[0], { status: 200, headers: { 'content-type': found[1] } });
};
const get = (path, method = 'GET') => worker.fetch(new Request(`http://get.duowave.app${path}`, { method }));

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

await check('the script, from the catalog over HTTPS, with its type', async () => {
    const answer = await get('/station.sh');
    assert.equal(answer.status, 200);
    assert.equal(answer.headers.get('content-type'), 'application/x-sh');
    assert.match(await answer.text(), /^#!\/bin\/sh/);
    assert.deepEqual(asked.at(-1), ['https://packages.duowave.app/station.sh', 'GET']);
});
await check('the build, whole', async () => {
    const answer = await get('/station/0.18.3/duowave-station-aarch64.tar.gz');
    assert.equal(answer.status, 200);
    assert.equal((await answer.arrayBuffer()).byteLength, 70_000);
});
await check("the catalog's 404 stays a 404", async () => {
    assert.equal((await get('/station/0.18.2/duowave-station-aarch64.tar.gz')).status, 404);
});
await check('everything else is 404 without asking the catalog', async () => {
    const before = asked.length;
    for (const path of ['/', '/catalog.json', '/index.html', '/station', '/station/', '/station.sh.bak', '/packages/x.vgp', '/station/a/b/c']) {
        assert.equal((await get(path)).status, 404, path);
    }
    assert.equal(asked.length, before);
});
await check('no way out of /station/', async () => {
    assert.equal(served('/station/../catalog.json'), false);
    assert.equal(served('/station/0.18.3/..'), false);
    assert.equal(served('/station/0.18.3/duowave-station-aarch64.tar.gz.sig'), true);
});
await check('HEAD answers, POST does not', async () => {
    const head = await get('/station.sh', 'HEAD');
    assert.equal(head.status, 200);
    assert.equal(asked.at(-1)[1], 'HEAD');
    assert.equal((await get('/station.sh', 'POST')).status, 405);
});

console.log(`${failures} failed`);
process.exit(failures);
