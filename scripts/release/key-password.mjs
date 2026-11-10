#!/usr/bin/env node
// Puts a password on the update key, or changes it, without changing the key: the same key pair and id, so every
// installation keeps trusting it — only the file is encrypted again. Tauri's key file is base64 of minisign's secret key
// text; minisign encrypts the secret part with scrypt of the password (the salt and the limits are in the file). This
// decrypts it with the current password, checks the result is a real key (its public half derives from its seed), and
// encrypts it with the new password and a fresh salt into <key>.with-password, mode 0600. The key file itself is left
// as it is: replace it by hand once the new one works (docs/plans/release-hardening.md). Node's built-in modules only.
//
//   node scripts/release/key-password.mjs ~/.tauri/duowave-updates.key
//
// Asks for the current password (Enter when there is none) and the new one twice, without echo. For a script or a test:
// KEY_PASSWORD_OLD and KEY_PASSWORD_NEW instead of the questions.
import { createPrivateKey, createPublicKey, randomBytes, scryptSync, timingSafeEqual } from 'node:crypto';
import { readFileSync, writeFileSync } from 'node:fs';
import readline from 'node:readline';

// minisign's secret key: sig_alg 2, kdf_alg 2, chk_alg 2, salt 32, opslimit 8, memlimit 8, then the 104 encrypted bytes
// (key id 8, secret key 64, checksum 32).
const LENGTH = 158;
const SALT = 6;
const OPS = 38;
const MEM = 46;
const SECRET = 54;
// minisign's own limits, for a key that had no encryption at all.
const OPSLIMIT = 1048576n;
const MEMLIMIT = 1073741824n;

/** The key file: whether it is Tauri's base64 form, minisign's comment line, and the 158 bytes. */
function read(file) {
    const outer = readFileSync(file, 'utf8');
    const tauri = !outer.trimStart().startsWith('untrusted comment:');
    const text = tauri ? Buffer.from(outer.trim(), 'base64').toString('utf8') : outer;
    const [comment, body] = text.split('\n').map((line) => line.trim()).filter(Boolean);
    const raw = Buffer.from(body ?? '', 'base64');
    if (!comment?.startsWith('untrusted comment:') || raw.length !== LENGTH || raw.subarray(0, 2).toString('latin1') !== 'Ed') {
        throw new Error(`${file}: not a minisign secret key`);
    }
    return { tauri, comment, raw, newline: outer.endsWith('\n') };
}

/** The 104 bytes scrypt gives for a password, with N, r, p chosen from the limits as libsodium chooses them. */
function stream(password, salt, opslimit, memlimit) {
    const ops = opslimit < 32768n ? 32768n : opslimit;
    const r = 8n;
    let log2 = 1n;
    let p = 1n;
    if (ops < memlimit / 32n) {
        const maxN = ops / (r * 4n);
        while (log2 < 63n && !(1n << log2 > maxN / 2n)) log2 += 1n;
    } else {
        const maxN = memlimit / (r * 128n);
        while (log2 < 63n && !(1n << log2 > maxN / 2n)) log2 += 1n;
        let maxrp = ops / 4n / (1n << log2);
        if (maxrp > 0x3fffffffn) maxrp = 0x3fffffffn;
        p = maxrp / r > 0n ? maxrp / r : 1n;
    }
    const N = Number(1n << log2);
    const memory = 128 * N * Number(r) * Number(p);
    return scryptSync(Buffer.from(password, 'utf8'), salt, 104, { N, r: Number(r), p: Number(p), maxmem: memory * 2 });
}

const xor = (data, key) => Buffer.from(data.map((byte, index) => byte ^ key[index]));

/** The secret part in the clear, or an error when the password does not open it. */
function open(key, password) {
    const kdf = key.raw.subarray(2, 4).toString('latin1');
    const sealed = key.raw.subarray(SECRET);
    let clear;
    if (kdf === '\0\0') clear = Buffer.from(sealed);
    else if (kdf === 'Sc') {
        const salt = key.raw.subarray(SALT, OPS);
        clear = xor(sealed, stream(password, salt, key.raw.readBigUInt64LE(OPS), key.raw.readBigUInt64LE(MEM)));
    } else throw new Error(`the key's encryption is unknown (${JSON.stringify(kdf)})`);
    // A wrong password gives noise: the public half of the secret key then does not come from its seed.
    const seed = clear.subarray(8, 40);
    const der = Buffer.concat([Buffer.from('302e020100300506032b657004220420', 'hex'), seed]);
    const derived = createPublicKey(createPrivateKey({ key: der, format: 'der', type: 'pkcs8' })).export({ format: 'jwk' }).x;
    if (!timingSafeEqual(Buffer.from(derived, 'base64url'), clear.subarray(40, 72))) throw new Error('the current password does not open the key');
    return clear;
}

/** The key file with `clear` sealed by `password` under a fresh salt. */
function seal(key, clear, password) {
    const raw = Buffer.from(key.raw);
    const encrypted = raw.subarray(2, 4).toString('latin1') === 'Sc';
    const opslimit = encrypted ? raw.readBigUInt64LE(OPS) : OPSLIMIT;
    const memlimit = encrypted ? raw.readBigUInt64LE(MEM) : MEMLIMIT;
    raw.write('Sc', 2, 'latin1');
    randomBytes(32).copy(raw, SALT);
    raw.writeBigUInt64LE(opslimit, OPS);
    raw.writeBigUInt64LE(memlimit, MEM);
    xor(clear, stream(password, raw.subarray(SALT, OPS), opslimit, memlimit)).copy(raw, SECRET);
    const text = `${key.comment}\n${raw.toString('base64')}\n`;
    const out = key.tauri ? Buffer.from(text, 'utf8').toString('base64') : text;
    return key.newline && !out.endsWith('\n') ? `${out}\n` : out;
}

/** A question whose answer is not shown as it is typed. */
function ask(question) {
    return new Promise((resolve) => {
        const rl = readline.createInterface({ input: process.stdin, output: process.stdout, terminal: true });
        rl._writeToOutput = () => {};
        process.stdout.write(question);
        rl.question('', (answer) => {
            rl.close();
            process.stdout.write('\n');
            resolve(answer);
        });
    });
}

const file = process.argv[2];
if (!file) {
    console.error('usage: key-password.mjs <key file>');
    process.exit(2);
}
try {
    const key = read(file);
    const current = process.env.KEY_PASSWORD_OLD ?? (await ask('Текущий пароль ключа (Enter, если его нет): '));
    const clear = open(key, current);
    const id = Buffer.from(clear.subarray(0, 8)).reverse().toString('hex').toUpperCase();
    let fresh = process.env.KEY_PASSWORD_NEW;
    if (fresh === undefined) {
        fresh = await ask(`Новый пароль для ключа ${id}: `);
        if ((await ask('Ещё раз: ')) !== fresh) throw new Error('пароли не совпали');
    }
    if (!fresh) throw new Error('пустой пароль: ключ остался бы без пароля');
    const target = `${file}.with-password`;
    writeFileSync(target, seal(key, clear, fresh), { mode: 0o600 });
    // The new file opens with the new password to the very same key.
    if (!open(read(target), fresh).equals(clear)) throw new Error(`${target}: the check after writing failed`);
    console.log(`${target}: ключ ${id} тот же, теперь с паролем. Старый файл не тронут.`);
} catch (error) {
    console.error(error.message);
    process.exit(1);
}
