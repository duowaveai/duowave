// Проверки приложения YouTube без сети: разбор ответов YouTube (образцы настоящих ответов, имена каналов заменены),
// выбор трека, слова о бедах, файл yt-dlp и его сумма, Range, очистка слов человека.
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';

import { parseRange } from '../lib/audio.mjs';
import { durationMs, parseMix, parseSearch, pick } from '../lib/innertube.mjs';
import { kindOf, sayError } from '../lib/words.mjs';
import { assetFor, sumFor } from '../lib/ytdlp.mjs';
import { cleanQuery } from '../server.mjs';

const fixture = (name) => JSON.parse(readFileSync(new URL(`./fixtures/${name}`, import.meta.url), 'utf8'));

test('разбирает поиск: номер, название, канал, длина; эфир — без длины', () => {
    const found = parseSearch(fixture('search.json'));
    assert.equal(found.length, 8);
    assert.deepEqual(found[0], { id: 'xKpzH5bxYsk', title: 'Виктор Цой - Группа Крови', channel: 'Канал 1', durationMs: 291_000 });
    assert.equal(found[1].durationMs, null, 'прямой эфир');
    assert.equal(parseSearch({}).length, 0, 'чужой ответ — пусто, не падение');
});

test('выбирает первое не длиннее 15 минут; сборник — только если его просили', () => {
    const found = parseSearch(fixture('search.json'));
    assert.equal(pick(found, 'кино группа крови').id, 'xKpzH5bxYsk');
    const long = found.filter((video) => video.durationMs > 15 * 60_000);
    assert.equal(pick([...long, found[0]], 'кино').id, 'xKpzH5bxYsk');
    assert.equal(pick([...long, found[0]], 'кино альбом').id, long[0].id);
    assert.equal(pick([found[1]], 'эфир'), null, 'эфир не играем');
});

test('разбирает микс RD<id>: само видео первым, дальше похожее', () => {
    const mix = parseMix(fixture('next.json'));
    assert.equal(mix[0].id, 'xtxjm7ciwmc');
    assert.equal(mix[1].title, 'Звезда по имени Солнце');
    assert.equal(mix[1].channel, 'Группа КИНО');
    assert.equal(mix[1].durationMs, 226_000);
});

test('длина: минуты и часы', () => {
    assert.equal(durationMs('4:45'), 285_000);
    assert.equal(durationMs('1:02:03'), 3_723_000);
    assert.equal(durationMs('LIVE'), null);
});

test('беда YouTube словами: блокировка, 429, шифр, нет видео, нет ffmpeg', () => {
    assert.equal(kindOf({ message: 'YouTube не ответил: ETIMEDOUT', network: true }), 'blocked');
    assert.equal(kindOf(new Error('ERROR: [youtube] x: Unable to download API page: <urlopen error timed out>')), 'blocked');
    assert.equal(kindOf({ message: 'YouTube ответил HTTP 429', status: 429 }), 'busy');
    assert.equal(kindOf(new Error("ERROR: [youtube] x: Sign in to confirm you're not a bot")), 'busy');
    assert.equal(kindOf(new Error('WARNING: nsig extraction failed: Some formats may be missing')), 'cipher');
    assert.equal(kindOf(new Error('ERROR: [youtube] x: Video unavailable')), 'gone');
    assert.equal(kindOf({ message: 'нет ffmpeg', kind: 'ffmpeg' }), 'ffmpeg');
    assert.match(sayError({ status: 429 }, 'ru'), /не пускает с вашего адреса/);
    assert.match(sayError({ network: true }, 'ru'), /Нужен VPN/);
    assert.match(sayError({ network: true }, 'en'), /needs a VPN/);
});

test('yt-dlp: файл под систему и его сумма из SHA2-256SUMS', () => {
    assert.deepEqual(assetFor('darwin', 'arm64'), { asset: 'yt-dlp_macos.zip', zip: true, program: 'yt-dlp_macos' });
    assert.equal(assetFor('linux', 'arm64').asset, 'yt-dlp_linux_aarch64');
    assert.equal(assetFor('linux', 'x64').asset, 'yt-dlp_linux');
    assert.equal(assetFor('win32', 'x64').asset, 'yt-dlp.exe');
    assert.equal(assetFor('freebsd', 'x64'), null);
    const sums = `${'a'.repeat(64)}  yt-dlp\n${'B'.repeat(64)}  yt-dlp_macos\n${'c'.repeat(64)}  yt-dlp_macos.zip\n`;
    assert.equal(sumFor(sums, 'yt-dlp_macos'), 'b'.repeat(64));
    assert.equal(sumFor(sums, 'yt-dlp_linux'), null);
    assert.equal(sumFor('zzz  yt-dlp_macos', 'yt-dlp_macos'), null, 'не сумма');
});

test('Range: с места, хвост, за концом', () => {
    assert.deepEqual(parseRange('bytes=100-', 1000), { start: 100, end: 999 });
    assert.deepEqual(parseRange('bytes=0-99', 1000), { start: 0, end: 99 });
    assert.deepEqual(parseRange('bytes=-100', 1000), { start: 900, end: 999 });
    assert.equal(parseRange('bytes=2000-', 1000).unsatisfiable, true);
    assert.equal(parseRange('items=1-2', 1000), null);
    assert.equal(parseRange(undefined, 1000), null);
});

test('слова человека без «с ютуба»', () => {
    assert.equal(cleanQuery('с ютуба кино группа крови'), 'кино группа крови');
    assert.equal(cleanQuery('кино группа крови на ютубе'), 'кино группа крови');
    assert.equal(cleanQuery('Кино из YouTube'), 'Кино');
    assert.equal(cleanQuery('ютуб'), '');
    assert.equal(cleanQuery('сплин'), 'сплин', 'слово с «с» в начале не трогаем');
});
