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

import { existsSync, readdirSync, readFileSync, statSync } from 'node:fs';
import { join, relative } from 'node:path';

const GATEWAY = /^(duowave-(?:station|darwin|linux|windows)-[a-z0-9_]+)\.tar\.gz$/;
const APP = /^duowave-app-((?:darwin|linux|windows)-(?:x86_64|aarch64))\.(?:app\.tar\.gz|exe|AppImage)$/;

/** Every file under a folder, as relative paths. */
function walk(dir) {
    return readdirSync(dir).flatMap((name) => {
        const path = join(dir, name);
        return statSync(path).isDirectory() ? walk(path).map((inner) => join(name, inner)) : [name];
    });
}

export function feed({ dir, version, base, notes, now = new Date() }) {
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
    return {
        version: version.replace(/^v/, ''),
        notes: notes ?? '',
        pub_date: now.toISOString().replace(/\.\d{3}Z$/, 'Z'),
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
