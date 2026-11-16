// Texts of the «Try saying» first-steps step (screens/onb/try.tsx). A demonstration: films, people and numbers are made up.
import type { Translation } from '../../types';
import type ru from '../ru/onbTry';

const en: Translation<typeof ru> = {
    title: 'Try saying',
    howComputer: 'Say it yourself or tap a line — I will show what it looks like. While DuoWave works, you can keep talking.',
    howStation: 'Say it to the speaker — the result shows up here, on the desk. Tap a line to see what it looks like.',
    wake: { station: 'Alice', computer: 'Deevi' },
    ask: (name: string, text: string) => `“${name}, ${text}”`,

    keyAlt: '⌥',
    keySpace: 'Space',
    keyBody: 'Hold it and talk, let go and DuoWave thinks. Or use the lime button in the dock.',
    stationChip: (name: string) => `“${name}, …”`,
    stationBody: 'Talk to the speaker as usual. Whatever gets done shows up here, on the desk.',

    stageLabel: 'How it happens',
    again: 'Again',

    demos: {
        widget: { phrase: 'make a widget with the dollar rate', kind: 'a widget in a minute — then you talk to it' },
        game: { phrase: 'make a game — a neon snake', kind: 'a game from a description — opens in a window' },
        search: { phrase: 'find what’s on at the cinema', kind: 'web search — answered by voice and a card' },
        memory: { phrase: 'remember: Mia is allergic to nuts', kind: 'memory — shared by all characters' },
        links: { phrase: 'what do you remember about Mia?', kind: 'links — who is who to whom' },
        sleep: {
            phrase: 'what did you figure out overnight?',
            phraseStation: 'what did you figure out overnight?',
            kind: 'sleep — at night DuoWave tidies up',
        },
    },

    status: {
        hearing: 'Listening…',
        busy: 'Working, you can keep talking',
        done: 'Done',
        thinking: 'Thinking…',
        asleep: 'Asleep · dreaming',
        awake: 'Awake',
        station: (status: string) => `Speaker · ${status.toLowerCase()}`,
    },

    widget: {
        drawing: 'New widget · drawing…',
        name: 'Dollar rate',
        kind: 'widget',
        price: '92.40 ₽',
        change: '▲ 0.3% today',
        euro: '€',
        euroPrice: '99.80 ₽',
        euroChange: '▼ 0.1%',
        making: 'Making the “Dollar rate” widget…',
        ready: 'Done. Put it on the desk?',
        toDesk: 'To the desk',
        tryOn: 'Try on',
        landed: 'The widget is on the desk. Now you can talk to it.',
        talk: 'add the euro and tell me if the dollar goes above 95',
        watch: 'Added the euro. Watching: I’ll tell you if the dollar goes above 95 ₽',
        helpTitle: 'You can talk to the widget',
        help1: 'what’s the rate now?',
        help1Tail: ' — a spoken answer from fresh data',
        help2: 'make it bigger',
        help2Tail: ' — remakes the widget: the new version lands on the desk in about a minute',
        help3: 'put it back',
        help3Tail: ' — brings the previous version back',
        helpNote: 'The rate updates by itself; the spoken answer uses fresh data.',
    },

    game: {
        workshop: 'Workshop · Neon snake',
        codeComment: '// 20×20 field, the snake grows from food',
        todo1: '20×20 field and food',
        todo2: 'Arrow-key controls',
        todo3: 'Score and best',
        todoCheck: 'Checking that it all works…',
        name: 'Neon snake',
        score: 'Score 3 · best 12',
        keys: 'Arrows to steer, space to pause',
        making: 'Making the “Neon snake” game…',
        done: 'Done: the game is in Apps.',
        play: 'Play',
    },

    search: {
        searching: 'Searching the web…',
        source1: 'city listings',
        source2: 'cinema websites',
        source3: 'audience reviews',
        note: 'Turn on web search in Apps → Connections: Exa or Apify — or pick a model with search.',
        title: 'Now showing',
        sources: '3 sources · Exa',
        film1: '“Quiet Harbour”',
        film1About: 'drama · 2 h 05 min',
        film2: '“The Star Postman”',
        film2About: 'animation · 1 h 32 min',
        film3: '“Last Ferry Out”',
        film3About: 'thriller · 1 h 58 min',
        reply: '“Three films are showing now. Want to hear about one?”',
        suggest: 'Turn this into a “Listings” widget?',
        make: 'Make it',
    },

    memory: {
        saved: 'Remembered: Mia is allergic to nuts',
        undo: 'Undo',
        ask: 'what should I cook for dinner?',
        reply: '“Pasta with vegetables and cheese — no nuts, so Mia can have it. Send the recipe?”',
        homeTitle: 'Memory lives at your home',
        homeBody:
            'It is kept and sorted on the speaker or the computer, not in someone else’s cloud. A cloud model gets only what it needs for the answer — you can see it in the Memory window. To send nothing at all, pick a model on the computer.',
        homeNote: 'Memory is shared by all characters at home. Any entry can be erased — “forget about the nuts”.',
    },

    links: {
        main: 'Mia · daughter',
        allergy: 'nut allergy · new',
        brother: 'brother — Pete',
        cat: 'cat Peach',
        birthday: 'birthday — March 3',
        infer: 'guess: gets up at 7 on weekdays',
        why: 'why: alarm at 7:00 five days in a row',
        reply: '“Mia is your daughter, she is allergic to nuts. Her brother is Pete, the cat is Peach, her birthday is March 3.”',
        more: 'Links — who is who to whom, with dates. Purple is a guess: accept or reject it in the Memory window.',
    },

    sleep: {
        time: '04:00 · everyone at home is asleep',
        dream: 'DuoWave is dreaming: going over the day.',
        row1: 'Merged repeats: “Mia doesn’t eat nuts” and “Mia has an allergy”',
        row2: 'Pete moved to a new school — closed the old one with a date',
        row3: '“On Friday” became a date: November 21',
        row4: 'Wrote the day’s summary in the diary',
        title: 'Overnight',
        where: 'Memory window',
        summary: '2 repeats merged · 1 fact outdated · a date instead of “on Friday”',
        infer: 'guess: Friday is pizza night',
        accept: 'Accept',
        reject: 'Reject',
        note: 'Sleep stays off until you turn it on: Memory → Settings. Any overnight change can be undone.',
    },

    phone: {
        widget: {
            title: 'The “Dollar rate” widget is on the desk',
            body: 'Then you talk to it: “what’s the rate now?”, “add the euro”, “make it bigger”.',
            grey: '',
        },
        game: {
            title: 'The game is in Apps',
            body: 'Opens on the phone with swipe controls. To change it — “make the snake faster”.',
            grey: '',
        },
        search: {
            title: 'A “Now showing” card',
            body: 'Three films with genre and length, plus a spoken answer.',
            grey: 'Turn on web search in Apps → Connections: Exa or Apify — or a model with search.',
        },
        memory: {
            title: 'Remembered',
            body: 'Later, “what should I cook for dinner?” gets an answer without nuts.',
            grey: 'Memory lives at home, on the speaker or the computer. A cloud model gets only what the answer needs.',
        },
        links: {
            title: 'Mia — daughter',
            body: 'Nut allergy · brother Pete · cat Peach · birthday March 3. Guess: gets up at 7 on weekdays.',
            grey: '',
        },
        sleep: {
            title: 'Overnight',
            body: '2 repeats merged, 1 fact outdated, “on Friday” became a date. The guess “Friday is pizza night” — accept or reject.',
            grey: 'Sleep stays off until you turn it on: Memory → Settings.',
        },
    },
};

export default en;
