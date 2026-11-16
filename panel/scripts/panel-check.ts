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
import { helpsSomeone, whereAmI } from '../src/shell/role.ts';
import type { PlacementHealth } from '../src/api.ts';
import { awayBackup, awayBehind } from '../src/screens/onb/brain-home.ts';
import { NO_BACKUP, backupIdle, backupOf, backupShown, blankBackups, pickValue } from '../src/screens/brain-backup.ts';
import { fallLine, fallOf, turboOf } from '../src/screens/turbo-model.ts';
import type { HelperView, HelpersView } from '../src/api.ts';

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

check('the_device_tabs_say_whose_panel_it_is_and_lead_to_the_other_in_one_click', () => {
    const station = { key: 'dev-1', name: 'Кухня', placement: 'stationServer' as const, address: '192.0.2.5', reachable: true };
    const onStation: PlacementHealth = {
        role: 'station',
        placement: 'stationServer',
        name: 'Кухня',
        peerApi: 3,
        helpers: { away: 'any', list: [{ id: 'mac', name: 'MacBook', kind: 'home', enabled: true, state: 'up', abilities: ['hearing'] }] },
    };
    const onComputer: PlacementHealth = { role: 'pc', placement: 'pc', peerApi: 3, stations: [station] };
    // The station's panel in the computer's window: the station pressed, «Этот компьютер · Турбо» one click away —
    // «Дом» to «Дом», anything else to the computer's desk.
    const via = whereAmI({ placement: onStation, role: { role: 'gateway', helping: true }, via: 'dev-1', computer: false, path: '/panel/', hash: '#/settings/helpers', current: 'Кухня' });
    expect(via.here === 'station' && via.tabs.length === 2 && via.tabs[0].on && via.tabs[0].tag === 'gateway', 'the station’s tab is pressed');
    expect(via.tabs[1].href === '/panel/?local#/settings/helpers' && via.tabs[1].tag === 'turbo', 'the computer’s own panel, «Дом» kept');
    const fromDesk = whereAmI({ placement: onStation, role: { role: 'gateway', helping: true }, via: 'dev-1', computer: false, path: '/panel/', hash: '#/character/brain' });
    expect(fromDesk.tabs[1].href === '/panel/?local', 'any other place leads to the computer’s desk');
    // The computer's own panel: each station it places, then the computer pressed.
    const own = whereAmI({ placement: onComputer, role: { role: 'helper', helping: true, station }, computer: false, path: '/panel/', hash: '' });
    expect(own.here === 'computer' && own.tabs.length === 2 && own.tabs[1].on && own.tabs[1].tag === 'turbo', 'the computer’s tab is pressed');
    expect(own.tabs[0].href === '/panel/?via=dev-1' && own.tabs[0].name === 'Кухня', 'the station’s panel through the computer');
    // A station opened in a browser: its Турбо shown, no address to go to; asleep — so said.
    const direct = whereAmI({ placement: onStation, role: { role: 'gateway', helping: true }, computer: false, path: '/panel/', hash: '' });
    expect(direct.tabs.length === 2 && direct.tabs[1].tag === 'turbo' && !direct.tabs[1].href, 'its Турбо without a link');
    const asleep = { ...onStation, helpers: { away: 'any' as const, list: [{ ...onStation.helpers!.list[0], state: 'down' as const }] } };
    expect(whereAmI({ placement: asleep, role: { role: 'gateway', helping: true }, computer: false, path: '/', hash: '' }).tabs[1].tag === 'asleep', 'an asleep Турбо');
    // «Микрофон»: one tab, the computer with its built-in Турбо.
    const mic: PlacementHealth = { role: 'pc', placement: 'pc', peerApi: 3, helpers: { away: 'home', list: [{ id: 'self', name: 'Этот компьютер', kind: 'home', builtin: true, enabled: true, state: 'self', abilities: ['hearing'] }] } };
    const alone = whereAmI({ placement: mic, role: { role: 'gateway', helping: false }, computer: true, path: '/', hash: '' });
    expect(alone.tabs.length === 1 && alone.tabs[0].tag === 'gatewayTurbo' && alone.here === 'computer', 'one tab: gateway + Турбо');
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
