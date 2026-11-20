// Проверки пульта без браузера. Чистая логика экранов — её модули без DOM: рисовать ли фон темы во фрейме пульта
// колонки (shell/backdrop-around.ts), уходят ли вопросы из дома за моделью на компьютере (screens/onb/brain-home.ts),
// помогает ли компьютер кому-то (shell/role.ts). Раскладка, которую без браузера не отрисовать, — по тексту исходников:
// у каждой вкладки «Настроек» есть страница, пароль другого устройства не помечен паролем входа сюда, и то, что нашла
// репетиция проверки руками блока «Дом». node ≥ 23.6 запускает файл как есть:
//
//     node panel/scripts/panel-check.ts        (или pnpm --filter panel check)

import { readFileSync, readdirSync } from 'node:fs';
import { dirname, join, relative } from 'node:path';
import { fileURLToPath } from 'node:url';

import type { BrainView, ProviderView } from '../src/api.ts';
import { paintedAround } from '../src/shell/backdrop-around.ts';
import { landingsOf, repairedLayout } from '../src/shell/desk-repair.ts';
import { helpsSomeone } from '../src/shell/role.ts';
import { chosenDevice, dotOf, placeDevice, thisDeviceOf, viewsOf } from '../src/home-model.ts';
import { endedStillShown } from '../src/shell/plaque-model.ts';
import { waitPage, waitShownOn } from '../src/voice/waiting-model.ts';
import type { WorkView } from '../src/api.ts';
import type { PlacementHealth } from '../src/api.ts';
import { awayBackup, awayBehind } from '../src/screens/onb/brain-home.ts';
import { NO_BACKUP, backupIdle, backupOf, backupShown, blankBackups, pickValue } from '../src/screens/brain-backup.ts';
import { fallLine, fallOf, turboOf } from '../src/screens/turbo-model.ts';
import type { HelperView, HelpersView, HomeDevice, HomeDevices } from '../src/api.ts';
import { type Laid, type Rect, arrange, overlaps, pushApart, underTop } from '../src/screens/desk-places.ts';
import { stationOfferOf, stationUpdatePressable, stationUpdateStep } from '../src/station-update.ts';
import type { StationUpdatePlan } from '../src/api.ts';

const checks: Array<[string, () => void]> = [];
const check = (name: string, run: () => void): void => void checks.push([name, run]);
function expect(condition: unknown, message: string): asserts condition {
    if (!condition) throw new Error(message);
}

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..');
const source = (path: string): string => readFileSync(join(ROOT, path), 'utf8');

/** Every .ts and .tsx file under `folder`. */
function sources(folder: string): string[] {
    return readdirSync(folder, { withFileTypes: true }).flatMap((entry) => {
        const path = join(folder, entry.name);
        if (entry.isDirectory()) return sources(path);
        return /\.tsx?$/.test(entry.name) ? [path] : [];
    });
}

// ─── The theme's backdrop in a page of the computer inside the station's panel (`?own=`) ─────────────────────────────

check('a_frame_skips_the_backdrop_only_when_the_page_around_paints_the_same', () => {
    const around = { backdrop: 'on', themeId: 'stars', theme: 'dark' };
    expect(paintedAround(around, 'stars', 'dark'), 'the same theme and mode around: a second layer of scenes in the frame');
    // «Примерить тему» in the workshop's frame: the station's panel does not know the draft and stands in Minimal.
    expect(!paintedAround({ themeId: 'minimal', theme: 'dark' }, 'preview-stars', 'dark'), 'a try-on is painted nowhere');
    expect(!paintedAround(around, 'preview-stars', 'dark'), 'another theme around is not this backdrop');
    expect(!paintedAround(around, 'stars', 'light'), 'another mode around is another backdrop');
    expect(!paintedAround({ themeId: 'stars', theme: 'dark' }, 'stars', 'dark'), 'the page around paints no backdrop');
    expect(!paintedAround(null, 'stars', 'dark'), 'a page of its own (no page around) draws its backdrop');
});

// ─── «Кто думает» of the first steps: do questions leave home behind the model on this computer ─────────────────────

const row = (id: string, title: string, cloud: boolean): ProviderView => ({
    id,
    preset: id,
    kind: 'openai',
    title,
    baseUrl: cloud ? null : 'http://127.0.0.1:11434/v1',
    options: {},
    key: { set: cloud },
    needsKey: cloud,
    editableUrl: !cloud,
    models: [],
    check: null,
    cloud,
    local: !cloud,
});

function brain(choice: Partial<BrainView['choice']>, envCloud = true): Pick<BrainView, 'choice' | 'providers' | 'env'> {
    return {
        providers: [row('ollama', 'Ollama', false), row('lmstudio', 'LM Studio', false), row('claude', 'Claude', true)],
        choice: { fast: null, smart: null, effort: 'medium', ...choice },
        env: { provider: 'openai', title: 'OpenAI', models: { fast: 'gpt-fast', smart: 'gpt-smart' }, usable: true, cloud: envCloud },
    };
}

const LOCAL = { provider: 'ollama', model: 'llama3' };

check('the_gateways_cloud_backup_behind_a_model_on_this_computer_is_named', () => {
    // Default 6 of block 1: the model on this computer became the main one, the gateway set a cloud backup itself.
    const auto = brain({ fast: LOCAL, backup: { provider: 'claude', model: 'claude-haiku-5-5' }, backupBy: 'gateway' });
    const behind = awayBehind(auto);
    expect(behind?.backup && behind.title === 'Claude' && behind.model === 'claude-haiku-5-5', 'the cloud backup is not named');
    expect(awayBackup(auto)?.title === 'Claude', 'the path card does not see the cloud backup');
    // One a person chose earlier is a cloud all the same.
    const chosen = brain({ fast: LOCAL, backup: { provider: 'claude', model: 'claude-haiku-5-5' } });
    expect(awayBehind(chosen)?.backup, 'a person’s cloud backup is not named');
});

check('questions_stay_home_without_a_cloud_behind', () => {
    expect(awayBehind(brain({ fast: LOCAL })) === null, 'nothing behind: the questions stay home');
    // «Без запасной»: the gateway remembers the no and sets none.
    expect(awayBehind(brain({ fast: LOCAL, backup: null, backupBy: 'declined' })) === null, 'declined: home');
    expect(awayBehind(brain({ fast: LOCAL, backup: { provider: 'lmstudio', model: 'qwen' } })) === null, 'a backup at home');
    expect(awayBehind(brain({ fast: LOCAL, backup: { provider: 'gone', model: 'x' } })) === null, 'a removed row answers nobody');
});

check('the_env_model_as_the_backup_is_away_only_in_the_cloud', () => {
    expect(awayBehind(brain({ fast: LOCAL, backup: { provider: 'env', model: 'gpt-fast' } }, true))?.title === 'OpenAI', 'cloud .env');
    expect(awayBehind(brain({ fast: LOCAL, backup: { provider: 'env', model: 'gpt-fast' } }, false)) === null, '.env at home');
});

check('a_cloud_smart_model_takes_the_hard_questions_out_and_is_no_backup', () => {
    const view = brain({ fast: LOCAL, smart: { provider: 'claude', model: 'claude-sonnet-5' } });
    const behind = awayBehind(view);
    expect(behind && !behind.backup && behind.model === 'claude-sonnet-5', 'a cloud smart model is not named');
    // «Без запасной» would not keep these in: the step offers it for a backup only.
    expect(awayBackup(view) === null, 'a smart model counted as the backup');
    // Both in the cloud: the words name the smart one, and the cloud backup is still there to be dropped.
    const both = brain({ ...view.choice, backup: { provider: 'claude', model: 'claude-haiku-5-5' } });
    expect(!awayBehind(both)?.backup && awayBackup(both)?.model === 'claude-haiku-5-5', 'both in the cloud: the backup is lost');
});

// ─── Places on screen ────────────────────────────────────────────────────────────────────────────────────────────────

check('every_tab_of_the_settings_screen_has_its_page_in_settings', () => {
    const settings = source('src/screens/settings.tsx');
    const places = source('src/screens/places.tsx');
    const union = /export type SettingsSectionTab = ([^;]+);/.exec(settings)?.[1] ?? '';
    const tabs = [...union.matchAll(/'([a-z]+)'/g)].map((match) => match[1]);
    expect(tabs.length >= 2, 'SettingsSectionTab is not found in settings.tsx');
    for (const tab of tabs) {
        const page = new RegExp(`\\{\\s*id: '${tab}',[^}]*?Page: (\\w+)`).exec(places)?.[1];
        expect(page, `«Настройки» have no page '${tab}' (places.tsx)`);
        const body = new RegExp(`function ${page}\\(\\)[^]*?\\n\\}`).exec(places)?.[0] ?? '';
        // «Оформление» once showed ThemeCard alone: the language, «Лента ярлыков», «Пройти первые шаги» had no place.
        expect(new RegExp(`<SettingsScreen[^>]*\\btab="${tab}"`).test(body), `the page '${tab}' (${page}) does not show the tab "${tab}"`);
    }
});

check('only_the_panels_own_sign_in_marks_a_field_as_this_sites_password', () => {
    const marked = sources(join(ROOT, 'src'))
        .filter((file) => readFileSync(file, 'utf8').includes('current-password'))
        .map((file) => relative(ROOT, file).split('\\').join('/'));
    // The speaker's panel password in «Помогать колонке» is another device's: a password manager must not offer this
    // computer's own there, nor save the speaker's in its place.
    const others = marked.filter((file) => file !== 'src/screens/auth.tsx');
    expect(others.length === 0, `fields marked current-password outside the sign-in: ${others.join(', ')}`);
    expect(marked.includes('src/screens/auth.tsx'), 'the sign-in no longer marks its password field');
});

// ─── What the rehearsal of the hand check found (block «Дом», 16.11) ────────────────────────────────────────────────

/** A function's text in a source file: from its line to the first closing brace at the line's start. */
function textOf(path: string, name: string): string {
    const found = new RegExp(`\\n(export )?function ${name}\\([^]*?\\n\\}`).exec(source(path))?.[0];
    expect(found, `${name} is not found in ${path}`);
    return found;
}

check('this_computer_stays_on_the_helpers_page_whatever_the_station_answers', () => {
    // In the station's window a 0.13.0 station answers the list with 404 and one away with an error: the page stopped
    // before «Этот компьютер», and «Подключить шлюз» was on no page of the window. Now the gateway's «Дом» (GatewayHome)
    // keeps the card leading to the computer's own «Дом».
    const page = textOf('src/screens/helpers.tsx', 'GatewayHome');
    for (const [why, path] of [
        ['an older station', /if \(older\)\s*return\s*\(([^]*?)\);/],
        ['a station away or still read', /if \(!view\)\s*return\s*\(([^]*?)\);/],
    ] as const) {
        expect(path.exec(page)?.[1].includes('<ThisComputerCard'), `${why}: «Дом» shows no «Этот компьютер — Турбо»`);
    }
    expect(/\?local#\/settings\/helpers/.test(textOf('src/screens/helpers.tsx', 'ThisComputerCard')), 'the card leads to the computer’s own «Дом»');
    // Its «Открыть пульт компьютера ↗» led from the station's window to the computer's /panel, which sends a helper
    // straight back to that window.
    const card = textOf('src/screens/desk-widgets.tsx', 'HelperCardWidget');
    expect(/!VIA/.test(card), 'the old computer card links the station window to itself');
});

check('the_brain_page_reads_again_on_the_gateways_news', () => {
    // The backup the gateway sets itself right after a choice showed up to 15 s later: «Мозг» read on a timer only.
    const tab = textOf('src/screens/brain.tsx', 'BrainTab');
    expect(/usePoll\(\{[^}]*version: panel\.version/.test(tab), '«Мозг» does not read on panel.version');
});

check('the_helpers_read_again_on_their_news_even_right_after_a_change', () => {
    // «Проверить сейчас» answered «не на связи», `helpers.up` came 0.7 s later and the read it asked for was skipped
    // as «read just now».
    const live = /useLive\([^]*?setVersion\(/.exec(source('src/app.tsx'))?.[0] ?? '';
    expect(live.includes('noteHelpersNews(name)'), 'the live channel does not mark the helpers stale before setVersion');
    const note = textOf('src/helpers.ts', 'noteHelpersNews');
    for (const name of ['helpers.up', 'helpers.down', 'helpers.changed']) {
        expect(note.includes(`'${name}'`), `${name} is not news of the helpers`);
    }
});

check('keep_awake_shows_only_while_this_computer_helps_someone', () => {
    // «Помогаю — <колонка> · работает сама» stood above «Не давать компьютеру спать» (its words are about a speaker
    // that leans on the computer).
    expect(!helpsSomeone([], 0), 'a lone computer helps nobody');
    expect(!helpsSomeone([{ placement: 'station' }], 0), 'a speaker that works alone leans on nobody');
    expect(helpsSomeone([{ placement: 'station' }, { placement: 'stationServer' }], 0), 'a speaker it helps');
    expect(helpsSomeone([], 1), 'a gateway connected by a key');
    const side = textOf('src/screens/helper-pc.tsx', 'HelperSide');
    expect(/helpsSomeone\([^)]*\) && (\(\s*<section[^>]*>\s*)?<KeepAwake/.test(side), '«Этот компьютер» shows the switch by another rule');
});

// ─── «Где я»: device tabs in the top bar (owner's decision 16.11, point 2) ───────────────────────────────────────────

check('the_home_bar_knows_which_device_this_is_and_each_device_has_its_own_tabs', () => {
    const kitchen: HomeDevice = { id: 'station:d1', kind: 'station', name: 'Кухня', roles: ['centre'], hears: true, state: 'online', copy: false, itself: true, gatewayId: 'g-st', device: 'd1' };
    const bedroom: HomeDevice = { id: 'station:d2', kind: 'station', name: 'Спальня', roles: [], hears: true, state: 'reconnecting', copy: false, device: 'd2' };
    const mac: HomeDevice = { id: 'turbo:mac', kind: 'computer', name: 'Кабинет', roles: ['turbo'], hears: true, state: 'asleep', copy: false, gatewayId: 'g-mac', order: 1 };
    const server: HomeDevice = { id: 'turbo:srv', kind: 'server', name: 'Сервер', roles: ['turbo'], hears: false, state: 'offline', copy: false, order: 2 };
    const home: HomeDevices = { home: { name: 'Дом' }, centre: kitchen.id, devices: [kitchen, bedroom, mac, server] };
    // In the computer's window the home's panel comes through the pass: «this» is the computer, by its gateway's id,
    // never the centre that answered; before its own /health is read — none.
    expect(thisDeviceOf(home, 'g-mac', true)?.id === 'turbo:mac', 'the computer finds itself');
    expect(thisDeviceOf(home, null, true) === undefined, 'nothing until its own id is read');
    // Opened at the centre itself (the app's window on a computer that is the centre): the one that answered; in a
    // phone's browser no device is «this».
    expect(thisDeviceOf(home, 'g-st', false, true)?.id === kitchen.id && thisDeviceOf(home, undefined, false, true)?.id === kitchen.id, 'the centre');
    expect(thisDeviceOf(home, 'g-st', false, false) === undefined, 'a phone is none of them');
    // The app's window on the Турбо computer itself (local, no pass): the list comes from the centre, its row first and
    // «itself» — the computer is still found by its own gateway's id, not the centre.
    expect(thisDeviceOf(home, 'g-mac', false, true)?.id === 'turbo:mac', 'the Турбо finds itself in its own window');
    // The tabs of a device page by its roles (the canvas 17.11).
    expect(viewsOf(kitchen, false).join() === 'overview,room,device', 'a centre speaker');
    expect(viewsOf(bedroom, false).join() === 'overview,room', 'a speaker that only hears');
    expect(viewsOf(mac, true).join() === 'overview,computer,homes,device', 'this computer');
    expect(viewsOf(mac, false).join() === 'overview,device', 'another computer: its own things are read at it');
    expect(viewsOf(server, false).join() === 'overview,device', 'a server');
    // The dots: online green, reconnecting yellow, asleep and offline grey.
    expect(dotOf(kitchen) === 'ok' && dotOf(bedroom) === 'warn' && dotOf(mac) === 'off' && dotOf(server) === 'off', 'the dots');
});

check('a_package_names_the_device_of_the_home_it_will_run_on', () => {
    const kitchen: HomeDevice = { id: 'station:d1', kind: 'station', name: 'Кухня', roles: ['centre'], hears: true, state: 'online', copy: false };
    const mac: HomeDevice = { id: 'turbo:mac', kind: 'computer', name: 'Кабинет', roles: ['turbo'], hears: true, state: 'asleep', copy: false };
    const both: HomeDevices = { home: { name: 'Дом' }, centre: kitchen.id, devices: [kitchen, mac] };
    expect(placeDevice('gateway', both).device?.id === kitchen.id, 'a plugin runs at the centre');
    expect(placeDevice('computer', both).device?.id === mac.id, 'an app for a computer runs on the Турбо');
    expect(placeDevice('station', both).device?.id === kitchen.id, 'an app for a speaker runs on the centre speaker');
    expect(placeDevice('network', both).network === true, 'a service out there');
    const alone: HomeDevices = { ...both, devices: [kitchen] };
    expect(placeDevice('computer', alone).missing === 'computer', 'no computer: so said');
    const pc: HomeDevice = { ...mac, id: 'gateway:g', roles: ['centre', 'turbo'] };
    const mic: HomeDevices = { home: { name: 'Дом' }, centre: pc.id, devices: [pc] };
    expect(placeDevice('computer', mic).device?.id === pc.id && placeDevice('station', mic).missing === 'station', 'a computer alone');
});

check('no_device_tabs_and_no_frame_by_role_remain', () => {
    const files = sources(join(ROOT, 'src')).map((path) => [relative(ROOT, path), readFileSync(path, 'utf8')] as const);
    for (const [path, text] of files) {
        expect(!/\bDeviceTabs\b|\bwhereAmI\b|\bapplyRole\b/.test(text), `${path}: the device tabs are gone (one panel)`);
    }
    expect(!/data-role='helper'/.test(source('src/ui/desk.css')), 'no dark frame for the Турбо panel');
});

// ─── «Если Турбо уснёт»: what stands in for its hearing and voice (owner's decision 16.11, point 3) ────────────────

check('when_the_turbo_sleeps_the_gateways_own_chain_stands_in', () => {
    const base = { station: true, mode: 'gateway' as const, away: 'any' as const, cloudEar: false, cloudVoice: false };
    // A speaker in «Алиса + ваш ИИ» without clouds: Yandex hears, Alice speaks, the mode stays.
    expect(fallOf('hearing', base).by === 'yandex' && !fallOf('hearing', base).modeFalls, 'Yandex hears in «Алиса + ваш ИИ»');
    // Clouds on keys that the regulator lets out stand in first; kept in («Только дома») they do not.
    const clouds = { ...base, cloudEar: true, cloudVoice: true };
    expect(fallOf('voice', clouds).by === 'cloud', 'the cloud voice on a key');
    expect(fallOf('hearing', { ...clouds, away: 'home' }).by === 'yandex', '«Только дома» keeps the cloud in');
    // «Ваш ИИ» holds only with both clouds; else the gateway falls back to «Алиса + ваш ИИ» itself.
    expect(fallOf('hearing', { ...base, mode: 'own' }).modeFalls, '«Ваш ИИ» falls without clouds');
    expect(!fallOf('voice', { ...clouds, mode: 'own' }).modeFalls, '«Ваш ИИ» holds on both clouds');
    expect(fallOf('voice', { ...base, mode: 'own', cloudEar: true }).modeFalls, 'one cloud is not enough');
    // A computer has no Yandex: nobody hears; in «Микрофон» the answer is shown as text.
    expect(fallOf('hearing', { ...base, station: false }).by === 'none', 'nobody hears on a computer without clouds');
    expect(fallOf('voice', { ...base, station: false, computer: true }).by === 'text', '«Микрофон» shows the answer as text');
});

check('the_scene_shows_the_turbo_that_works_and_the_built_in_one_on_a_lone_computer', () => {
    const helper = (id: string, more: Partial<HelperView> = {}): HelperView => ({
        id, name: id, kind: 'home', enabled: true, without: [], state: 'up', allowed: true,
        abilities: [{ id: 'model', ready: true }, { id: 'hearing', ready: true }, { id: 'voice', ready: true }], ...more,
    });
    const row = (ability: 'model' | 'hearing' | 'voice', by: 'helper' | 'here' | 'yandex', id?: string) => ({ ability, by, helper: id, first: true });
    // Two Турбо: the scene shows the one giving now, not merely the first listed.
    const two: HelpersView = { helpers: [helper('a', { state: 'down' }), helper('b')], now: [row('model', 'helper', 'b'), row('hearing', 'helper', 'b')], away: 'any' };
    const shown = turboOf(two);
    expect(shown?.helper.id === 'b' && shown.up && !shown.builtin && shown.gives.join() === 'model,hearing', 'the giving Турбо');
    // None gives now (all asleep): the first enabled, shown asleep.
    const asleep = turboOf({ ...two, now: [row('hearing', 'yandex')] });
    expect(asleep?.helper.id === 'a' && !asleep.up, 'the first one, asleep');
    // A lone computer: its built-in Турбо, while it gives hearing, the model or the voice; nothing else — no Турбо.
    const own: HelpersView = { helpers: [helper('self', { builtin: true, state: 'self' })], now: [row('model', 'helper', 'self')], away: 'home' };
    expect(turboOf(own)?.builtin === true, 'the built-in Турбо');
    expect(turboOf({ ...own, now: [row('model', 'here')] }) === undefined, 'no Турбо when it gives nothing');
    // The line under the scene: only for a Турбо online; the backup thinks, Yandex hears, Alice speaks.
    const input = { station: true, mode: 'gateway' as const, away: 'any' as const, cloudEar: false, cloudVoice: false, backup: true };
    const line = fallLine({ ...shown!, gives: ['model', 'hearing', 'voice'] }, input);
    expect(line?.items.map((item) => `${item.ability}:${item.by}`).join() === 'model:backup,hearing:yandex,voice:yandex', 'the line names each stand-in');
    expect(fallLine(asleep, input) === null && fallLine(turboOf(own), input) === null, 'no line for a sleeping or built-in Турбо');
    expect(fallLine({ ...shown!, gives: ['model'] }, { ...input, backup: false })?.items[0].by === 'alice', 'no backup: Alice thinks on a speaker');
});

// ─── «Запасная»: «пусть выберет шлюз» and «не нужна» are two answers (owner's decision 18) ──────────────────────────

check('the_backup_tells_let_the_gateway_choose_from_not_needed', () => {
    const choice = (more: Partial<BrainView['choice']>): BrainView['choice'] => ({ fast: null, smart: null, effort: 'medium', ...more });
    const pick = { provider: 'claude', model: 'claude-haiku-5-5' };
    // Nothing chosen, nothing declined: the gateway may choose — the list stands on «пусть выберет шлюз», sent as 'auto'.
    expect(backupShown(choice({ autoBackup: true })) === '', 'not chosen reads as «пусть выберет шлюз»');
    expect(backupOf('', true) === 'auto', '«пусть выберет шлюз» hands the choice back to the gateway');
    // «не нужна» is its own answer: shown as such, sent as null (the gateway remembers the refusal).
    expect(backupShown(choice({ autoBackup: true, backupBy: 'declined' })) === NO_BACKUP, 'a refusal reads as «не нужна»');
    expect(backupOf(NO_BACKUP, true) === null, '«не нужна» is a refusal');
    // The gateway's own pick stands on «пусть выберет шлюз», which names the model: picking that item chooses anew,
    // picking the model in its provider's group makes it the person's.
    const gateways = choice({ autoBackup: true, backup: pick, backupBy: 'gateway' });
    expect(backupShown(gateways) === '', 'the gateway’s own pick reads as «пусть выберет шлюз»');
    expect(blankBackups(gateways)[0].model === pick.model, '«пусть выберет шлюз» names the model it chose');
    // A person's model is that model.
    expect(backupShown(choice({ autoBackup: true, backup: pick })) === pickValue(pick), 'a person’s model');
    const back = backupOf(pickValue(pick), true);
    expect(back !== null && back !== 'auto' && back.provider === pick.provider && back.model === pick.model, 'the model back');
    // Which word goes with which value: the empty item hands the choice to the gateway, NO_BACKUP refuses.
    const blanks = blankBackups(choice({ autoBackup: true }));
    expect(
        blanks.length === 2 && blanks[0].value === '' && blanks[0].word === 'optGatewayChooses' && blanks[1].value === NO_BACKUP,
        '«пусть выберет шлюз» is the empty item',
    );
    expect(blanks[1].word === 'optNotNeeded' && !blanks[0].model, '«не нужна» is NO_BACKUP, and nothing chosen names no model');
    for (const blank of blanks) {
        const sent = backupOf(blank.value, true);
        expect(blank.word === 'optGatewayChooses' ? sent === 'auto' : sent === null, `${blank.word} sends what it says`);
    }
    // A gateway without the block (0.13.0, 0.14.0 — the station's window) knows only «не нужна»: never 'auto'.
    const older = blankBackups(choice({ backupBy: 'declined' }));
    expect(older.length === 1 && older[0].word === 'optNotNeeded' && older[0].value === '', 'one «не нужна» there');
    expect(backupShown(choice({ backupBy: 'declined' })) === '' && backupOf('', false) === null, 'and it sends null');
    expect(backupShown(choice({ backup: pick })) === pickValue(pick), 'its backup is a model as before');
    // «Пусть выберет шлюз» beside a cloud main model sets nothing: the page says so (no line beside a helper's model,
    // a model chosen, «не нужна», or a gateway without the block).
    expect(backupIdle(choice({ autoBackup: true }), false), 'a cloud main model: «Сейчас запасной нет…»');
    expect(!backupIdle(choice({ autoBackup: true }), true), 'a helper’s main model: the gateway sets one');
    expect(!backupIdle(choice({ autoBackup: true, backup: pick, backupBy: 'gateway' }), false), 'one is set');
    expect(!backupIdle(choice({ autoBackup: true, backupBy: 'declined' }), false), '«не нужна» says it itself');
    expect(!backupIdle(choice({}), false), 'a gateway without the block');
    const brain = source('src/screens/brain.tsx');
    expect(/blankBackups\(choice\)\.map/.test(brain), '«Запасная» takes its items without a model from blankBackups');
    expect(/backupIdle\(/.test(brain) && /tr\.brain\.backupGatewayIdle/.test(brain), 'the idle line is on the page');
});

check('a_widget_landed_under_its_package_main_goes_to_the_tile_the_desk_draws', () => {
    // Item 47: «На стол» by voice and by its button put `package.<id>.main`; the app's card on the desk is `.card.main`.
    const layout = { v: 1 as const, hidden: [], added: ['package.neon-snake.tile', 'package.btc.main'], places: { 'package.btc.main': { x: 10, y: 20 } }, order: [], fresh: { 'package.btc.main': 1 } };
    const widgets = [{ key: 'package.neon-snake.tile' }, { key: 'package.btc.card.main' }];
    const fixed = repairedLayout(layout, widgets);
    expect(fixed !== null, 'repaired');
    expect(fixed.added.join() === 'package.neon-snake.tile,package.btc.card.main', `added: ${fixed.added.join()}`);
    expect(fixed.places['package.btc.card.main']?.x === 10 && !fixed.places['package.btc.main'], 'its place goes with it');
    expect(fixed.fresh?.['package.btc.card.main'] === 1, 'still «новый»');
    expect(repairedLayout(fixed, widgets) === null, 'nothing more to repair');
    // Its real tile placed already (from «Добавить виджет»): the stray key is left, the real one not doubled.
    const both = { ...layout, added: ['package.btc.main', 'package.btc.card.main'] };
    expect(repairedLayout(both, widgets) === null, 'the real tile is already there');
    // A package whose tile really is `main` is left as it is.
    expect(repairedLayout(layout, [{ key: 'package.btc.main' }]) === null, 'a real main stays');
});

// Group Б of the owner's bug list of 19.11: widgets jumped left and right when one was put on the desk or taken off,
// and one hid under another. The desk as desk-canvas.tsx draws it: scene in the middle, columns 60 points from it.
const DESK = {
    columns: [-640, 340, -980, 680],
    width: 300,
    top: -260,
    gap: 16,
    sight: { left: -1250, right: 1250, bottom: 600 },
    guess: 200,
    addHeight: 140,
};
const SCENE: Rect = { x: -280, y: -260, w: 560, h: 456 };
/** Lay out, then keep what the desk gave: places as the layout would store them. */
function laidOut(widgets: Laid[], fixed: Rect[] = [SCENE]): { places: Record<string, { x: number; y: number; w?: number; h?: number }>; add: { x: number; y: number } } {
    const done = arrange(widgets, fixed, DESK);
    const places = Object.fromEntries(widgets.flatMap((widget) => (widget.place ? [[widget.key, widget.place]] : [])));
    return { places: { ...places, ...done.fresh }, add: done.add };
}
const rectsOf = (places: Record<string, { x: number; y: number; w?: number }>, heights: Record<string, number>): Array<Rect & { key: string }> =>
    Object.entries(places)
        .filter(([key]) => (heights[key] ?? 0) > 0)
        .map(([key, place]) => ({ key, x: place.x, y: place.y, w: place.w ?? 300, h: heights[key] }));
function apart(rects: Rect[], what: string): void {
    for (const [i, a] of rects.entries()) for (const b of rects.slice(i + 1)) expect(!overlaps(a, b), `${what}: ${JSON.stringify(a)} lies on ${JSON.stringify(b)}`);
}

check('a_widget_put_on_the_desk_or_taken_off_moves_no_other', () => {
    const heights: Record<string, number> = { btc: 470, radio: 200, notes: 260 };
    const first = laidOut(Object.keys(heights).map((key) => ({ key, height: heights[key] })));
    expect(Object.keys(first.places).length === 3, 'each got a place');
    apart([SCENE, ...rectsOf(first.places, heights)], 'first layout');
    // Bitcoin and the radio stand at the speaker, left and right, from the top — as the columns stood.
    expect(first.places.btc.x === -640 && first.places.btc.y === -260, `btc at ${JSON.stringify(first.places.btc)}`);
    // Put a new one on: the others stay where they were.
    heights.weather = 180;
    const more = laidOut([...Object.keys(first.places).map((key) => ({ key, place: first.places[key], height: heights[key] })), { key: 'weather', height: 180 }]);
    for (const key of Object.keys(first.places)) expect(JSON.stringify(more.places[key]) === JSON.stringify(first.places[key]), `${key} moved when weather came`);
    apart([SCENE, ...rectsOf(more.places, heights)], 'with weather');
    // Take the radio off (its place goes with it): the others stay; «Новый виджет» shows where the next one goes.
    const { radio: _gone, ...left } = more.places;
    void _gone;
    const less = laidOut(Object.keys(left).map((key) => ({ key, place: left[key], height: heights[key] })));
    for (const key of Object.keys(left)) expect(JSON.stringify(less.places[key]) === JSON.stringify(left[key]), `${key} moved when the radio left`);
    apart([SCENE, ...rectsOf(less.places, heights), { ...less.add, w: 300, h: 140 }], '«Новый виджет» on a free place');
    // A widget that is still loading (measured, but short) does not squeeze into a narrow gap it would outgrow.
    const gap = laidOut([{ key: 'a', place: { x: -640, y: -260 }, height: 100 }, { key: 'b', place: { x: -640, y: -40 }, height: 300 }, { key: 'new', height: 60 }]);
    expect(!(gap.places.new.x === -640 && gap.places.new.y < -40), `the newcomer went into a gap of 104: ${JSON.stringify(gap.places.new)}`);
});

check('the_new_widget_tile_stays_and_the_new_widget_lands_on_it', () => {
    // «Новый виджет» stands where the next widget will go; it does not jump when a place higher up frees, and the new
    // widget (a draft landed from the workshop) takes exactly its place.
    const kept = { x: 340, y: -260 };
    const widgets: Laid[] = [{ key: 'btc', place: { x: -640, y: -260 }, height: 300 }];
    expect(JSON.stringify(arrange(widgets, [SCENE], { ...DESK, keep: kept }).add) === JSON.stringify(kept), 'the tile stays');
    const landed = arrange([...widgets, { key: 'snake', height: 260 }], [SCENE], { ...DESK, keep: kept });
    expect(JSON.stringify(landed.fresh.snake) === JSON.stringify({ x: 340, y: -260, w: 300, h: 260 }), `landed at ${JSON.stringify(landed.fresh.snake)}`);
    // Something hangs over the desk there (the Turbo card): nothing new goes under it.
    const covered = arrange(widgets, [SCENE], { ...DESK, cover: { x: 300, y: -400, w: 700, h: 300 } });
    expect(covered.add.y >= -100 + 16 || covered.add.x < 300, `the tile went under the card: ${JSON.stringify(covered.add)}`);
});

check('a_widget_that_shows_up_later_never_lies_on_another', () => {
    // Fresh Hip-Hop & Rap landed while the music widget hid (nothing played); then the music began.
    const heights: Record<string, number> = { music: 0, hiphop: 160 };
    const landed = laidOut([{ key: 'music', height: 0 }, { key: 'hiphop', height: 160 }]);
    expect(!landed.places.music, 'a hidden widget takes no place yet');
    heights.music = 200;
    const played = laidOut([{ key: 'music', height: 200 }, { key: 'hiphop', place: landed.places.hiphop, height: 160 }]);
    expect(JSON.stringify(played.places.hiphop) === JSON.stringify(landed.places.hiphop), 'Fresh Hip-Hop stays');
    apart([SCENE, ...rectsOf(played.places, heights)], 'music came');
    // The music stops (hidden again): its place is held, a new widget does not go there.
    heights.music = 0;
    const kept = laidOut([{ key: 'music', place: played.places.music, height: 0 }, { key: 'hiphop', place: played.places.hiphop, height: 160 }, { key: 'clock', height: 150 }]);
    apart([SCENE, { ...played.places.music, w: 300, h: played.places.music.h ?? 0 }, ...rectsOf(kept.places, { hiphop: 160, clock: 150 })], 'the place of the hidden music');
    // An old desk where one already lies on another (the owner's screenshot): the lower one moves down, under it.
    const moved = pushApart([{ key: 'music', x: -640, y: -260, w: 300, h: 200 }, { key: 'hiphop', x: -640, y: -250, w: 300, h: 160 }], [], 16);
    expect(!moved.music && moved.hiphop?.y === -44, `moved: ${JSON.stringify(moved)}`);
});

check('a_widget_that_grows_moves_down_only_the_one_it_lies_on', () => {
    const places = { btc: { x: -640, y: -260 }, notes: { x: -640, y: 30 }, far: { x: -980, y: -260 }, right: { x: 340, y: -260 } };
    // Bitcoin's data came: 270 → 470 points; it now lies on «notes» below it, not on the others.
    const moved = pushApart(rectsOf(places, { btc: 470, notes: 200, far: 300, right: 300 }), [], 16);
    expect(JSON.stringify(Object.keys(moved)) === '["notes"]' && moved.notes.y === 226, `moved: ${JSON.stringify(moved)}`);
    // The showcase under the scene stands under the widgets that begin above its spot and reach into it.
    expect(underTop(250, 454, [{ x: -640, y: -260, w: 300, h: 560 }], 20) === 320, 'under the near column');
    expect(underTop(250, 454, [{ x: -980, y: -260, w: 300, h: 900 }], 20) === 250, 'a far column is not in its way');
});

check('a_widget_put_on_the_desk_by_voice_past_the_fitting_lands_on_the_desk', () => {
    // Check 19.11: «да» to «Готово. На стол или примерить?» installed the app on the gateway, which said «На столе.», but
    // the desk is the panel's — no tile sat there.
    const widgets = [{ key: 'package.hiphop.card.main' }, { key: 'package.btc.card.main' }, { key: 'core.memory' }];
    const layout = { added: ['package.btc.card.main'], hidden: [] as string[] };
    const keys = landingsOf(['hiphop', 'btc'], layout, widgets);
    expect(keys.join() === 'package.hiphop.card.main', `lands the one not there: ${keys.join()}`);
    // Taken off the desk by the person: it stays off.
    expect(landingsOf(['hiphop'], { added: [], hidden: ['package.hiphop.card.main'] }, widgets).length === 0, 'taken off — not back');
    // Landed by the fitting under `main` (repairedLayout moves it): not landed twice.
    expect(landingsOf(['hiphop'], { added: ['package.hiphop.main'], hidden: [] }, widgets).length === 0, 'landed already under main');
    // The app's part not loaded yet: wait for the tile, no key the desk does not know.
    expect(landingsOf(['fresh'], { added: [], hidden: [] }, widgets).length === 0, 'no tile yet — nothing');
    const desk = source('src/screens/desk.tsx');
    expect(/landingsOf\(drafts\.installed/.test(desk) && /actions\.land\(key\)/.test(desk), 'the desk lands them');
});

// ─── Hand check 19.11: the header follows the device page, «Ждёт вас» always opens what waits, the plaque's «Готово» ──

check('the_home_bar_lights_the_device_whose_page_is_open_and_else_this_one', () => {
    const ids = ['hall', 'mac'];
    const at = (section: string, tab?: string, item?: string) => ({ section, tab, item });
    expect(chosenDevice(at('settings', 'devices', 'hall'), ids, 'mac') === 'hall', 'the page of «Гостиная» open: it is lit, not this Mac');
    expect(chosenDevice(at('settings', 'devices', 'mac'), ids, 'mac') === 'mac', 'back to the Mac: the Mac');
    expect(chosenDevice(at('settings', 'devices'), ids, 'mac') === 'mac', 'the list of devices: this device');
    expect(chosenDevice(at('home'), ids, 'mac') === 'mac', 'elsewhere: this device');
    expect(chosenDevice(at('settings', 'devices', 'gone'), ids, 'mac') === 'mac', 'a device not in the home: this device');
    expect(chosenDevice(at('skills', 'devices', 'hall'), ids, undefined) === undefined, 'another section with the same tab is not the page');
    const bar = source('src/shell/home-bar.tsx');
    expect(/chosenDevice\(panel\.link/.test(bar) && !/mine\?\.id === device\.id/.test(bar), 'the bar lights the chosen device, not only this one');
});

check('a_work_waiting_is_shown_on_the_strip_only_when_the_strip_can_and_else_its_page_opens', () => {
    const work = (fields: Partial<WorkView>) => ({ kind: 'app', target: 'btc', result: { line: 'Bitcoin rate: готово.' }, ...fields }) as WorkView;
    expect(waitShownOn(work({ question: { text: '', until: 0, kind: 'ready' } })) === 'strip', '«Готово» with its draft: the strip');
    expect(waitShownOn(work({ question: { text: '', until: 0, kind: 'unfinished' } })) === 'page', '«Доделать?» has its own buttons: the page');
    expect(waitShownOn(work({ question: { text: '', until: 0, kind: 'apply' } })) === 'page', '«Применить?»: the page');
    expect(waitShownOn(work({ result: undefined, question: { text: '', until: 0, kind: 'ready' } })) === 'page', 'no line to show: the page');
    expect(waitShownOn(work({ target: undefined, question: { text: '', until: 0, kind: 'ready' } })) === 'page', 'no draft: the page');
    expect(waitShownOn(work({ kind: 'tool', target: undefined, question: { text: '', until: 0, kind: 'more' } })) === 'strip', 'a tool’s result: the strip');
    expect(waitPage({ id: '7', target: 'btc' }, true)?.section === 'works' && waitPage({ id: '7', target: 'btc' }, true)?.tab === '7', 'the admin: the work in «Работы»');
    expect(waitPage({ id: '7', target: 'btc' }, false)?.item === 'btc', 'no «Работы»: the draft in the workshop');
    expect(waitPage({ id: '7' }, false) === null, 'nothing to open without either');
    const pill = source('src/voice/waiting.tsx');
    expect(/await reload\(\)/.test(pill), 'a press reads the list afresh: the number and what is behind it are one');
    expect(/panel\.go\(page\.section/.test(pill), 'what the strip cannot show opens its page');
});

check('the_plaque_drops_a_done_job_once_its_thing_is_decided', () => {
    // The gateway’s clock is 1000 ms ahead of ours: the job ended at 51000 there, 50000 here.
    expect(endedStillShown(51_000, 1000, undefined), 'nothing decided: «Готово» stays its minute and a half');
    expect(!endedStillShown(51_000, 1000, 50_500), 'installed after it ended: off the plaque');
    expect(endedStillShown(51_000, 1000, 49_000), 'decided before this run ended: this run’s end is news');
    const plaque = source('src/shell/agent-plaque.tsx');
    expect(/endedStillShown\(job\.ended, own\.offset, decided\.get\(job\.draft\)\)/.test(plaque), 'the plaque filters its own jobs');
});

check('an_applied_theme_is_the_theme_the_panel_keeps', () => {
    const fitting = source('src/screens/fitting.tsx');
    expect(/takeDraftTheme\(id\)/.test(fitting), '«Включить» on a theme takes it onto the panel');
    expect(/currentTheme\(\)\.id === tried\.id\) setTheme\(tried\.before\)/.test(fitting), 'closed without applying: the theme before it, not the default');
    const workshop = source('src/screens/workshop.tsx');
    expect(/currentTheme\(\)\.id\.startsWith\('preview-'\)\) setTheme\(previewing\)/.test(workshop), 'the workshop gives back only a try-on still shown');
    const works = source('src/screens/works.tsx');
    expect(/takeThemeWhenInstalled\(work\.target\)/.test(works), '«Применить» in «Работы» takes the theme once installed');
});

// ─── «Обновить колонку» (owner 20.11): «Доступна 0.16.3» — и «У колонки последняя версия» ───────────────────────────

check('the_station_window_asks_the_computer_to_update_itself_first_when_it_is_behind', () => {
    // 20.11: the station saw 0.16.3 in the feed, the computer ran 0.16.2 — and gives the station only its own build.
    const behind = stationUpdateStep({ plan: { way: 'computerFirst', version: '0.16.3' } }, '0.16.3');
    expect(behind?.kind === 'computerFirst' && behind.version === '0.16.3', 'the button updates the computer and then the station');
    expect(stationUpdatePressable(behind), 'there is a button');
    expect(stationOfferOf(behind) === '0.16.3', 'the dot says there is a version for the station');
    // The computer is ahead: its own build goes now.
    const ahead = stationUpdateStep({ plan: { way: 'send', version: '0.16.3' } }, '0.16.3');
    expect(ahead?.kind === 'send' && ahead.version === '0.16.3', 'the computer sends its build');
});

check('the_station_follows_the_computer_with_nothing_to_press', () => {
    const plan: StationUpdatePlan = { way: 'computerFirst', version: '0.16.3' };
    const follows = stationUpdateStep({ plan, afterComputer: '0.16.3' }, '0.16.3');
    expect(follows?.kind === 'follows' && follows.version === '0.16.3', 'the computer is updating, the station follows');
    expect(!stationUpdatePressable(follows), 'no second button that would start it again');
});

check('a_version_the_computer_cannot_give_says_why_and_offers_no_button', () => {
    const why = 'На этом компьютере DuoWave 0.16.2, а колонке он ставит только свою версию.';
    const blocked = stationUpdateStep({ plan: { way: 'blocked', version: '0.16.3', why } }, '0.16.3');
    expect(blocked?.kind === 'blocked' && blocked.why === why, 'the computer’s words');
    expect(!stationUpdatePressable(blocked), 'no button that would answer «последняя версия»');
    const ahead = stationUpdateStep({ plan: { way: 'stationAhead', station: '0.16.4', computer: '0.16.3' } }, null);
    expect(ahead?.kind === 'stationAhead' && !stationUpdatePressable(ahead), 'the station newer than the computer: the computer is to update');
    expect(stationOfferOf(ahead) === null, 'nothing new for the station');
});

check('latest_only_when_nothing_newer_is_known_anywhere', () => {
    // The station saw a version, and the computer has not said anything yet: the button asks it, it decides.
    const asked = stationUpdateStep({}, '0.16.3');
    expect(asked?.kind === 'ask' && stationUpdatePressable(asked), 'asked of the computer');
    expect(stationUpdateStep({}, null) === null, 'nothing anywhere: latest');
    expect(stationUpdateStep(null, undefined) === null, 'no view, nothing seen: latest');
});

check('the_panel_shows_the_computers_way_not_only_its_own_build', () => {
    const house = source('src/screens/house.tsx');
    expect(/stationUpdateStep\(computer, offer\?\.version\)/.test(house), 'the station window decides by the computer’s view');
    expect(/api\.updateStationFromComputer\(VIA, offer\?\.version\)/.test(house), 'it says the computer which version the station saw');
    expect(/!VIA && status\.canInstallAlone/.test(house), '«Поставить без компьютера» — only in the station’s own panel');
    const places = source('src/screens/places.tsx');
    expect(/\{VIA && <Updates own title=\{tr\.core\.tabThisComputer\} \/>\}/.test(places), 'the window of the home shows the computer’s own updates');
    const devices = source('src/screens/devices.tsx');
    expect(/mine && VIA && !centre \? <Updates own \/>/.test(devices), 'the computer’s tab shows its own version, not the station’s');
    for (const screen of ['src/screens/placement.tsx', 'src/screens/desk-widgets.tsx']) {
        expect(/stationUpdateStep\(station\)/.test(source(screen)), `${screen}: the computer’s plan, not only its own build`);
    }
});

check('the_pairs_of_an_app_are_looked_up_in_the_catalog_of_the_device_it_went_to', () => {
    // 20.11: installing at the home's centre through the computer, the pair was offered (and removed) by the computer's
    // own catalog. The pairs look where the app went: the actions' `installApi(station)`.
    const catalog = source('src/screens/catalog.tsx');
    expect(/async function catalogNow\(at: CatalogApi\)/.test(catalog), 'the catalog is the one given');
    expect(!/catalogNow\(\)/.test(catalog), 'never this panel’s own by default');
    expect(/offerPairs\(entry, install, say, api\)/.test(catalog) && /tr\.catalog\.removedNote\), api\)/.test(catalog), 'both pass the actions’ api');
});

check('an_app_for_both_is_placed_where_its_gateway_is', () => {
    // DuoGram runs on a station and on a computer: on a station's gateway the gateway says `station` (catalog.rs,
    // place_of) — the card shows that station, not «нужен компьютер-Турбо» of a home without a Турбо.
    const station = { id: 's', kind: 'station', roles: [] } as unknown as HomeDevice;
    const view = { centre: 's', devices: [station] } as unknown as HomeDevices;
    expect(placeDevice('station', view).device === station, 'the station that is the centre');
    expect(placeDevice('computer', view).missing === 'computer', 'an app for a computer still asks for one');
    for (const file of ['src/magicplace/section.tsx', 'src/magicplace/wizard.tsx']) {
        expect(source(file).includes('entry.runs, entry.place)'), `${file} passes the gateway’s place to its word`);
    }
    expect(source('src/screens/plugins.tsx').includes('view.runs, view.place)'), 'the installed app’s word too');
});

let failed = 0;
for (const [name, run] of checks) {
    try {
        run();
        console.log(`ok   ${name}`);
    } catch (error) {
        failed++;
        console.log(`FAIL ${name}: ${(error as Error).message}`);
    }
}
console.log(failed ? `${failed} of ${checks.length} failed` : `all ${checks.length} passed`);
process.exitCode = failed ? 1 : 0;
