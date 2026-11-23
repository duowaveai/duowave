// Demo wiring only: words, sample lines and buttons around the library. English is the source; Russian is a
// translation of it, kept in the same shape so a third language is one more entry in WORDS.

import {
    type BodyKind,
    type Character,
    MOOD_IDS,
    type Mascot,
    type MascotOptions,
    type MoodId,
    createMascot,
    deevi,
    drawnMood,
    isMood,
    lookOf,
} from '../src/index.ts';
import { sample } from './sample-character.ts';

type Lang = 'en' | 'ru';
type Want = NonNullable<MascotOptions['body']>;

/** The demo's characters: Deevi, and the sample that shows what a new one needs. */
const CAST: Character[] = [deevi, sample];

interface Words {
    title: string;
    cast: string;
    who: Record<string, string>;
    moods: string;
    pair: string;
    sizes: string;
    soundOn: string;
    soundOff: string;
    otherLang: string;
    wants: Record<Want, string>;
    bodyName: Record<BodyKind, string>;
    drawing: (body: string) => string;
    stage: (who: string) => string;
    note: string;
    names: Record<MoodId, string>;
    lines: Partial<Record<MoodId, [who: string, text: string]>>;
}

const WORDS: Record<Lang, Words> = {
    en: {
        title: 'DuoWave mascot',
        cast: 'Character',
        who: { deevi: 'Deevi', sample: 'Sample' },
        moods: 'Mood',
        pair: 'Same mood, light and rich body',
        sizes: 'Sizes (auto)',
        soundOn: 'Sound on',
        soundOff: 'Sound off',
        otherLang: 'Русский',
        wants: { auto: 'Auto', light: 'Light', rich: 'Rich' },
        bodyName: { light: 'Light body', rich: 'Rich body' },
        drawing: (body) => `Drawing: ${body}`,
        stage: (who) => `${who}. Tap to react, drag to turn.`,
        note: 'The light body shows at once. Deevi’s rich body is 3D: it loads only into boxes of 96 px and up, at most four at once. The sample character has no rich body and no look for some moods; an arrow shows what it shows instead.',
        names: {
            idle: 'Waiting',
            tender: 'Touched',
            wink: 'Winking',
            sleep: 'Asleep',
            listen: 'Listening',
            think: 'Thinking',
            speak: 'Speaking',
            success: 'Done',
            surprise: 'Surprised',
            sad: 'Sad',
        },
        lines: {
            tender: ['You', '“Tell me a story about a dragon and Mia”'],
            wink: ['DuoWave', 'On it. I’ll tell you when it’s ready.'],
            listen: ['You', '“Add milk to the list”'],
            think: ['DuoWave', 'Building the skill and testing it in a sandbox…'],
            speak: ['Claude Code', 'Tests passed. Ready to ship.'],
            success: ['DuoWave', 'Skill installed ✓'],
            surprise: ['DuoWave', 'Whoa, the render is done already!'],
            sad: ['DuoWave', 'I didn’t catch that. Say it again?'],
        },
    },
    ru: {
        title: 'Маскот DuoWave',
        cast: 'Персонаж',
        who: { deevi: 'ДиВи', sample: 'Образец' },
        moods: 'Состояние',
        pair: 'Одно состояние, лёгкое и полное тело',
        sizes: 'Размеры (авто)',
        soundOn: 'Звук вкл.',
        soundOff: 'Звук выкл.',
        otherLang: 'English',
        wants: { auto: 'Авто', light: 'Лёгкое', rich: 'Полное' },
        bodyName: { light: 'Лёгкое тело', rich: 'Полное тело' },
        drawing: (body) => `Рисует: ${body}`,
        stage: (who) => `${who}. Нажмите, чтобы отозвался, тяните, чтобы повернуть.`,
        note: 'Лёгкое тело появляется сразу. Полное тело ДиВи объёмное: оно грузится только в окошки от 96 пикселей, не больше четырёх одновременно. У образца нет полного тела и нет вида для некоторых состояний, стрелка показывает, что он рисует вместо них.',
        names: {
            idle: 'Ждёт',
            tender: 'Умиляется',
            wink: 'Подмигивает',
            sleep: 'Спит',
            listen: 'Слушает',
            think: 'Думает',
            speak: 'Говорит',
            success: 'Готово',
            surprise: 'Удивлён',
            sad: 'Грустит',
        },
        lines: {
            tender: ['Вы', '«Расскажи сказку про дракона и Мию»'],
            wink: ['DuoWave', 'Сделаю. Скажу, когда будет готово.'],
            listen: ['Вы', '«Добавь молоко в список»'],
            think: ['DuoWave', 'Собираю навык и проверяю его в песочнице…'],
            speak: ['Claude Code', 'Тесты прошли. Можно выкатывать.'],
            success: ['DuoWave', 'Навык установлен ✓'],
            surprise: ['DuoWave', 'Ого, рендер уже готов!'],
            sad: ['DuoWave', 'Не расслышал. Повторите?'],
        },
    },
};

const $ = (id: string): HTMLElement => {
    const found = document.getElementById(id);
    if (!found) throw new Error(`demo: no #${id}`);
    return found;
};

const params = new URLSearchParams(location.search);
let lang: Lang = params.get('lang') === 'ru' ? 'ru' : 'en';
let want: Want = (['auto', 'light', 'rich'] as const).find((b) => b === params.get('body')) ?? 'auto';
let mood: MoodId = isMood(params.get('mood')) ? (params.get('mood') as MoodId) : 'idle';
let cast: Character = CAST.find((c) => c.id === params.get('character')) ?? deevi;
let sound = false;

const stage = $('stage');
const pairLight = $('lightStage');
const pairRich = $('richStage');
let main: Mascot | null = null;
const pair: Mascot[] = [];
const small: Mascot[] = [];

const words = (): Words => WORDS[lang];
const who = (): string => words().who[cast.id] ?? cast.id;

function paintStage(el: HTMLElement): void {
    el.style.setProperty('--mood', lookOf(cast, mood).color);
}

function showMood(animate: boolean): void {
    const w = words();
    main?.setMood(mood, animate);
    for (const m of [...pair, ...small]) m.setMood(mood, animate);
    for (const el of [stage, pairLight, pairRich, ...document.querySelectorAll<HTMLElement>('#sizes .stage')]) paintStage(el);
    $('stateLabel').textContent = w.names[mood];
    const line = w.lines[mood];
    const says = $('says');
    if (line) {
        $('saysWho').textContent = line[0];
        $('saysText').textContent = line[1];
        says.classList.remove('off');
    } else says.classList.add('off');
    for (const chip of document.querySelectorAll<HTMLButtonElement>('#moods button'))
        chip.setAttribute('aria-pressed', String(chip.dataset.id === mood));
}

/** Which body each stage draws: the rich one arrives a moment later, or never. */
function showBodies(): void {
    const w = words();
    $('bodyLabel').textContent = w.drawing(main ? w.bodyName[main.body] : '');
    $('lightCaption').textContent = pair[0] ? w.bodyName[pair[0].body] : '';
    $('richCaption').textContent = pair[1] ? w.bodyName[pair[1].body] : '';
}
const showBodiesSoon = (): void => {
    showBodies();
    window.setTimeout(showBodies, 600);
    window.setTimeout(showBodies, 2000);
};

function mountMain(): void {
    main?.dispose();
    main = createMascot(stage, {
        character: cast,
        mood,
        body: want,
        sound,
        onMoodChange: (id) => {
            // A tap wakes a sleeping mascot: keep the buttons in step.
            if (id !== mood) {
                mood = id;
                showMood(false);
            }
        },
    });
}

function mountSide(): void {
    for (const m of pair.splice(0)) m.dispose();
    pair.push(
        createMascot(pairLight, { character: cast, mood, body: 'light' }),
        createMascot(pairRich, { character: cast, mood, body: 'rich' }),
    );
    const sizes = $('sizes');
    for (const m of small.splice(0)) m.dispose();
    sizes.replaceChildren();
    for (const size of [40, 64, 96, 140]) {
        const box = document.createElement('div');
        box.className = 'stage';
        box.style.width = `${size}px`;
        box.style.height = `${Math.round(size * 1.1)}px`;
        paintStage(box);
        sizes.appendChild(box);
        small.push(createMascot(box, { character: cast, mood }));
    }
}

function chips<T extends string>(
    id: string,
    items: readonly T[],
    label: (item: T) => string,
    pressed: (item: T) => boolean,
    pick: (item: T) => void,
    kind = 'toggle',
): void {
    $(id).replaceChildren(
        ...items.map((item) => {
            const chip = document.createElement('button');
            chip.type = 'button';
            chip.className = kind;
            chip.dataset.id = item;
            chip.textContent = label(item);
            chip.setAttribute('aria-pressed', String(pressed(item)));
            chip.addEventListener('click', () => pick(item));
            return chip;
        }),
    );
}

function render(): void {
    const w = words();
    document.documentElement.lang = lang;
    document.title = w.title;
    $('title').textContent = w.title;
    $('castHeading').textContent = w.cast;
    $('moodHeading').textContent = w.moods;
    $('pairHeading').textContent = w.pair;
    $('sizesHeading').textContent = w.sizes;
    $('note').textContent = w.note;
    $('lang').textContent = w.otherLang;
    $('sound').textContent = sound ? w.soundOn : w.soundOff;
    stage.setAttribute('aria-label', w.stage(who()));

    chips(
        'cast',
        CAST.map((c) => c.id),
        (id) => w.who[id] ?? id,
        (id) => id === cast.id,
        (id) => {
            cast = CAST.find((c) => c.id === id) ?? deevi;
            render();
            mountMain();
            mountSide();
            showMood(false);
            showBodiesSoon();
        },
    );
    // A mood the character has no look for shows where it falls back to.
    chips(
        'moods',
        MOOD_IDS,
        (id) => {
            const drawn = drawnMood(cast, id);
            return drawn === id ? w.names[id] : `${w.names[id]} → ${w.names[drawn]}`;
        },
        (id) => id === mood,
        (id) => {
            mood = id;
            showMood(true);
        },
        'chip',
    );
    chips(
        'bodies',
        ['auto', 'light', 'rich'] as const,
        (b) => w.wants[b],
        (b) => b === want,
        (b) => {
            want = b;
            render();
            mountMain();
            showBodiesSoon();
        },
    );
    showMood(false);
    showBodies();
}

$('lang').addEventListener('click', () => {
    lang = lang === 'en' ? 'ru' : 'en';
    render();
});
$('sound').addEventListener('click', () => {
    sound = !sound;
    main?.setSound(sound);
    $('sound').setAttribute('aria-pressed', String(sound));
    render();
});

mountMain();
mountSide();
render();
showBodiesSoon();
