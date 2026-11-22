// get.duowave.app: the station's one-line install over plain HTTP (`wget -qO- http://get.duowave.app/station.sh | sh`,
// station-client/station.sh). The station's busybox has no TLS, and the catalog's site (packages.duowave.app, GitHub
// Pages) answers https:// only. This Worker takes two kinds of paths from the catalog over HTTPS and hands them on:
//   /station.sh                                   the release's script, its version and the build's SHA-256 inside
//   /station/<version>/<file>                     the station build it names and its .sig
// Everything else is 404. The catalog's own build put these files there only after the build's signature held
// (duowave-packages, tools/station.sh); the station then checks the build against the script's checksum.
//
// The body is read whole and handed on with its exact Content-Length: a streamed body would go out chunked, and the
// runtime ignores a Content-Length set by hand (developers.cloudflare.com/workers/runtime-apis/response, 22.11.2026).
// With a length, busybox wget knows a download cut short. The largest file is the station build, about 8 MB.
// No dependencies; how to put it on Cloudflare: README.md beside it.

const UPSTREAM = 'https://packages.duowave.app';
const SCRIPT = '/station.sh';
const BUILD = /^\/station\/[0-9A-Za-z][0-9A-Za-z.+-]*\/[0-9A-Za-z][0-9A-Za-z._-]*$/;

/** Whether this path is one the station asks for. */
export function served(path) {
    return path === SCRIPT || (BUILD.test(path) && !path.includes('..'));
}

export default {
    async fetch(request) {
        if (request.method !== 'GET' && request.method !== 'HEAD') {
            return new Response('only GET and HEAD\n', { status: 405, headers: { allow: 'GET, HEAD' } });
        }
        const path = new URL(request.url).pathname;
        if (!served(path)) return new Response('not found\n', { status: 404, headers: { 'content-type': 'text/plain' } });
        const upstream = await fetch(UPSTREAM + path, { method: request.method, redirect: 'follow' });
        const headers = new Headers({ 'cache-control': 'no-cache' });
        const type = upstream.headers.get('content-type');
        if (type) headers.set('content-type', type);
        if (request.method === 'HEAD') return new Response(null, { status: upstream.status, headers });
        // An ArrayBuffer: the runtime sets Content-Length from it.
        return new Response(await upstream.arrayBuffer(), { status: upstream.status, headers });
    },
};
