#!/usr/bin/env node
// Checks a release's signed files before anything builds on them or publishes them: each file's minisign signature
// (the .sig beside it — Tauri's base64 of minisign's text) by a key the installations trust, and for the gateway's
// archives the release.json inside them — the tag's version and the platform the archive is named for. The gateway
// refuses an archive without release.json or with another version (gateway/src/updates/install.rs); this stops such a
// release here, before it reaches anyone. The desktop app's update files are checked against the one key the app
// trusts (tauri.conf.json), everything else against gateway/updates.pub. Node's built-in modules only: no package
// runs beside a release's files.
//
//   node scripts/release/verify-sig.mjs [--version vX.Y.Z] <file>...    each <file> with <file>.sig beside it
//   --keys <file>   other keys than updates.pub, one per line, for every file (a test's throwaway key)
import { createHash, createPublicKey, verify } from 'node:crypto';
import { existsSync, readFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { gunzipSync } from 'node:zlib';

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', '..');
const GATEWAY_ARCHIVE = /^(duowave-(?:station|darwin|linux|windows)-[a-z0-9_]+)\.tar\.gz$/;
const APP_FILE = /^duowave-app-/;

/** minisign's text, from Tauri's base64 of it or as it is. */
function minisignText(raw) {
    const text = raw.trim();
    return text.startsWith('untrusted comment:') ? text : Buffer.from(text, 'base64').toString('utf8');
}

/** A key's id as minisign prints it: the eight bytes as a little-endian number, in hex. */
const keyId = (bytes) => Buffer.from(bytes).reverse().toString('hex').toUpperCase();

/** A public key from one line: Tauri's base64 of the .pub text, or minisign's bare base64 key line. */
function publicKey(line) {
    const text = line.startsWith('RW') ? line : minisignText(line);
    const keyLine = text
        .split('\n')
        .map((part) => part.trim())
        .find((part) => part && !part.startsWith('untrusted comment:'));
    const bytes = Buffer.from(keyLine ?? '', 'base64');
    if (bytes.length !== 42 || bytes.subarray(0, 2).toString('latin1') !== 'Ed') throw new Error(`not a minisign key: ${line.slice(0, 24)}…`);
    const jwk = { kty: 'OKP', crv: 'Ed25519', x: bytes.subarray(10).toString('base64url') };
    return { id: keyId(bytes.subarray(2, 10)), key: createPublicKey({ key: jwk, format: 'jwk' }) };
}

/** The keys the gateway trusts: updates.pub, one per non-empty line (a rotation adds its key beside the last). */
function gatewayKeys(file = path.join(ROOT, 'gateway/updates.pub')) {
    return readFileSync(file, 'utf8')
        .split('\n')
        .map((line) => line.trim())
        .filter((line) => line && !line.startsWith('untrusted comment:'))
        .map(publicKey);
}

/** The one key the desktop app trusts: plugins.updater.pubkey in tauri.conf.json. */
function appKeys() {
    const config = JSON.parse(readFileSync(path.join(ROOT, 'desktop/src-tauri/tauri.conf.json'), 'utf8'));
    return [publicKey(config.plugins.updater.pubkey)];
}

/** A .sig file: its algorithm, key id, signature, trusted comment and the signature over that comment. */
function signatureOf(file) {
    const lines = minisignText(readFileSync(file, 'utf8')).split('\n').map((line) => line.replace(/\r$/, ''));
    const [untrusted, signatureLine, trustedLine, globalLine] = lines;
    if (!untrusted?.startsWith('untrusted comment:') || !trustedLine?.startsWith('trusted comment: ') || !globalLine) {
        throw new Error(`${path.basename(file)}: not a minisign signature`);
    }
    const signature = Buffer.from(signatureLine, 'base64');
    if (signature.length !== 74) throw new Error(`${path.basename(file)}: not a minisign signature`);
    return {
        algorithm: signature.subarray(0, 2).toString('latin1'),
        id: keyId(signature.subarray(2, 10)),
        signature: signature.subarray(10),
        trusted: trustedLine.slice('trusted comment: '.length),
        global: Buffer.from(globalLine, 'base64'),
    };
}

/** Whether `file` is signed, as the gateway and the app check it: prehashed (BLAKE2b-512), by one of `keys`. */
function checkSignature(file, keys) {
    const sigFile = `${file}.sig`;
    if (!existsSync(sigFile)) throw new Error(`${path.basename(file)}: no .sig beside it`);
    const signed = signatureOf(sigFile);
    if (signed.algorithm !== 'ED') throw new Error(`${path.basename(file)}: not a prehashed signature (${signed.algorithm})`);
    const key = keys.find((candidate) => candidate.id === signed.id);
    if (!key) throw new Error(`${path.basename(file)}: signed by key ${signed.id}, not one the installations trust (${keys.map((k) => k.id).join(', ')})`);
    const digest = createHash('blake2b512').update(readFileSync(file)).digest();
    if (!verify(null, digest, key.key, signed.signature)) throw new Error(`${path.basename(file)}: the signature does not match the file`);
    const comment = Buffer.concat([signed.signature, Buffer.from(signed.trusted, 'utf8')]);
    if (!verify(null, comment, key.key, signed.global)) throw new Error(`${path.basename(file)}: the trusted comment is not the signer's`);
    return key.id;
}

/** release.json inside a .tar.gz, at the top or in one folder, as the gateway finds it; null when there is none. */
export function releaseOf(archive) {
    const tar = gunzipSync(readFileSync(archive));
    for (let offset = 0; offset + 512 <= tar.length; ) {
        const header = tar.subarray(offset, offset + 512);
        if (header.every((byte) => byte === 0)) break;
        const field = (start, length) => header.subarray(start, start + length).toString('utf8').replace(/\0[\s\S]*$/, '');
        // POSIX ustar keeps a path's beginning at 345; GNU's own format («ustar  ») keeps other fields there.
        const prefix = header.subarray(257, 263).toString('latin1') === 'ustar\0' ? field(345, 155) : '';
        const name = prefix ? `${prefix}/${field(0, 100)}` : field(0, 100);
        const size = parseInt(field(124, 12).trim() || '0', 8);
        const type = header[156];
        const body = offset + 512;
        const parts = name.split('/').filter((part) => part && part !== '.');
        if ((type === 0x30 || type === 0) && parts.at(-1) === 'release.json' && parts.length <= 2) {
            return JSON.parse(tar.subarray(body, body + size).toString('utf8'));
        }
        offset = body + Math.ceil(size / 512) * 512;
    }
    return null;
}

function main(args) {
    const option = (name) => {
        const at = args.indexOf(name);
        return at >= 0 ? args.splice(at, 2)[1] : undefined;
    };
    const version = option('--version')?.replace(/^v/, '');
    const keysFile = option('--keys');
    if (!args.length) {
        console.error('usage: verify-sig.mjs [--version vX.Y.Z] [--keys file] <file>...');
        return 2;
    }
    const keys = keysFile ? { gateway: gatewayKeys(keysFile), app: gatewayKeys(keysFile) } : { gateway: gatewayKeys(), app: appKeys() };
    const problems = [];
    for (const file of args) {
        const name = path.basename(file);
        try {
            const id = checkSignature(file, APP_FILE.test(name) ? keys.app : keys.gateway);
            const archive = GATEWAY_ARCHIVE.exec(name);
            if (!archive) {
                console.log(`ok ${name}: key ${id}`);
                continue;
            }
            const release = releaseOf(file);
            if (!release) throw new Error(`${name}: no release.json inside — the gateway would refuse it`);
            if (release.platform !== archive[1]) throw new Error(`${name}: release.json says ${release.platform}`);
            if (version && release.version !== version) throw new Error(`${name}: release.json says ${release.version}, the tag ${version}`);
            console.log(`ok ${name}: key ${id}, ${release.version} for ${release.platform}`);
        } catch (error) {
            problems.push(error.message);
        }
    }
    if (problems.length) {
        console.error(`not signed as a release must be:\n  ${problems.join('\n  ')}`);
        return 1;
    }
    return 0;
}

// Run as a program (not when imported): compared as paths, which a folder name with spaces does not break.
if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) process.exit(main(process.argv.slice(2)));
