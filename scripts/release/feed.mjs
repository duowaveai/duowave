#!/usr/bin/env node
// The release feed, latest.json: one file for the desktop app (Tauri's keys: darwin-aarch64, …) and the gateway alone
// (duowave-<os>-<arch>, duowave-station-<arch>), each with its archive's URL and minisign signature
// (gateway/src/updates). Reads a folder of release assets and, if there is one, the app's own latest.json from
// tauri-action, and writes the merged feed.
//
//   node scripts/release/feed.mjs <assets-dir> <version> <download-base-url> [notes-file] > latest.json
//
// A gateway archive is duowave-<key>.tar.gz beside duowave-<key>.tar.gz.sig (`tauri signer sign` writes the .sig). The
// app's update archive is duowave-app-<os>-<arch>.<app.tar.gz|exe|AppImage> beside its .sig (release.yml names them so):
// it goes under Tauri's own key, <os>-<arch>.
//
// An urgent release: critical.txt beside this script lists the versions that fix something urgent, and the feed carries
// the highest of them up to this release as critical_below — a gateway older than that installs this release by
// itself (gateway/src/updates). Tauri's updater and gateways before 0.2.1 skip the field.

import { existsSync, readdirSync, readFileSync, statSync } from 'node:fs';
import { join, relative } from 'node:path';
import { fileURLToPath } from 'node:url';

const GATEWAY = /^(duowave-(?:station|darwin|linux|windows)-[a-z0-9_]+)\.tar\.gz$/;
const APP = /^duowave-app-((?:darwin|linux|windows)-(?:x86_64|aarch64))\.(?:app\.tar\.gz|exe|AppImage)$/;
const VERSION = /^v?(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z.-]+))?$/;

/** Two versions as semver orders them: numbers first, then a prerelease below its release. */
export function compareVersions(a, b) {
    const [left, right] = [VERSION.exec(a), VERSION.exec(b)];
    if (!left || !right) throw new Error(`not a version: ${left ? b : a}`);
    for (let index = 1; index <= 3; index += 1) {
        const difference = Number(left[index]) - Number(right[index]);
        if (difference) return Math.sign(difference);
    }
    if (!left[4] || !right[4]) return left[4] ? -1 : right[4] ? 1 : 0;
    const [ours, theirs] = [left[4].split('.'), right[4].split('.')];
    for (let index = 0; index < Math.max(ours.length, theirs.length); index += 1) {
        const [x, y] = [ours[index], theirs[index]];
        if (x === undefined || y === undefined) return x === undefined ? -1 : 1;
        if (x === y) continue;
        const [xn, yn] = [/^\d+$/.test(x), /^\d+$/.test(y)];
        if (xn && yn) return Math.sign(Number(x) - Number(y));
        if (xn !== yn) return xn ? -1 : 1;
        return x < y ? -1 : 1;
    }
    return 0;
}

/** The versions critical.txt lists: the first word of each line, # starts a comment. A word that is no version fails. */
export function criticalVersions(file = fileURLToPath(new URL('critical.txt', import.meta.url))) {
    if (!existsSync(file)) return [];
    return readFileSync(file, 'utf8')
        .split('\n')
        .map((line) => line.replace(/#.*/, '').trim().split(/\s+/)[0])
        .filter(Boolean)
        .map((version) => {
            if (!VERSION.test(version)) throw new Error(`${file}: «${version}» is not a version`);
            return version.replace(/^v/, '');
        });
}

/** critical_below for a release: the highest urgent version up to its own, or none. */
export function criticalBelow(version, critical) {
    const upTo = critical.filter((urgent) => compareVersions(urgent, version) <= 0).sort(compareVersions);
    return upTo.at(-1);
}

/** Every file under a folder, as relative paths. */
function walk(dir) {
    return readdirSync(dir).flatMap((name) => {
        const path = join(dir, name);
        return statSync(path).isDirectory() ? walk(path).map((inner) => join(name, inner)) : [name];
    });
}

export function feed({ dir, version, base, notes, critical = criticalVersions(), now = new Date() }) {
    const files = walk(dir);
    const platforms = {};
    // The app's keys first, from tauri-action's own latest.json when the release has one.
    const appFeed = files.find((file) => file.endsWith('latest.json'));
    if (appFeed) {
        const app = JSON.parse(readFileSync(join(dir, appFeed), 'utf8'));
        Object.assign(platforms, app.platforms ?? {});
    }
    for (const file of files) {
        const name = file.split('/').pop();
        const match = GATEWAY.exec(name) ?? APP.exec(name);
        if (!match) continue;
        const sigFile = `${file}.sig`;
        if (!files.includes(sigFile)) throw new Error(`${file}: no ${name}.sig beside it — sign it first`);
        platforms[match[1]] = {
            signature: readFileSync(join(dir, sigFile), 'utf8').trim(),
            url: `${base.replace(/\/+$/, '')}/${name}`,
        };
    }
    if (!Object.keys(platforms).length) throw new Error(`no signed archives in ${relative(process.cwd(), dir) || dir}`);
    const below = criticalBelow(version, critical);
    return {
        version: version.replace(/^v/, ''),
        notes: notes ?? '',
        pub_date: now.toISOString().replace(/\.\d{3}Z$/, 'Z'),
        ...(below ? { critical_below: below } : {}),
        platforms,
    };
}

if (import.meta.url === `file://${process.argv[1]}`) {
    const [dir, version, base, notesFile] = process.argv.slice(2);
    if (!dir || !version || !base) {
        console.error('usage: feed.mjs <assets-dir> <version> <download-base-url> [notes-file]');
        process.exit(2);
    }
    const notes = notesFile && existsSync(notesFile) ? readFileSync(notesFile, 'utf8').trim() : undefined;
    process.stdout.write(`${JSON.stringify(feed({ dir, version, base, notes }), null, 2)}\n`);
}
