// metadata.duowave.app: DuoWave's OAuth client document (CIMD, SEP-991). An MCP server's authorization server that
// has no client registration (ElevenLabs' cloud MCP) reads it: the document's own address is the client id. The
// gateway sends that address (CLIENT_METADATA_URL, crates/duowave-apps/src/mcp/mcp_oauth.rs), and every release has it
// built in, so the address, the document's client_id and the path below stay one and the same.
//
//   GET/HEAD /oauth/client-metadata.json   200, the document as JSON, cached an hour, no redirect
//   any other path                         404
//   any other method                       405
//
// The document lives here, not on the landing site: a site's rebuild or move must not sign every user out of their
// MCP servers. It lists one return, the gateway's callback on its default port 7780.
// No dependencies; how to put it on Cloudflare: README.md beside it.

export const PATH = '/oauth/client-metadata.json';
export const CLIENT_ID = `https://metadata.duowave.app${PATH}`;

/** The document's exact bytes, as served. */
export const DOCUMENT = `{
  "client_id": "${CLIENT_ID}",
  "client_name": "DuoWave",
  "client_uri": "https://duowave.ai",
  "logo_uri": "https://duowave.ai/icon-512.png",
  "redirect_uris": ["http://localhost:7780/api/v1/mcp/oauth/callback"],
  "grant_types": ["authorization_code", "refresh_token"],
  "response_types": ["code"],
  "token_endpoint_auth_method": "none",
  "application_type": "native"
}
`;

export default {
    async fetch(request) {
        if (request.method !== 'GET' && request.method !== 'HEAD') {
            return new Response('only GET and HEAD\n', { status: 405, headers: { allow: 'GET, HEAD' } });
        }
        if (new URL(request.url).pathname !== PATH) {
            return new Response('not found\n', { status: 404, headers: { 'content-type': 'text/plain' } });
        }
        const headers = { 'content-type': 'application/json', 'cache-control': 'public, max-age=3600' };
        return new Response(request.method === 'HEAD' ? null : DOCUMENT, { status: 200, headers });
    },
};
