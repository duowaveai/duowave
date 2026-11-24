// __TITLE__: учитель игры на MIDI-клавиатуре (docs/plugins/AUTHORING.md, «MIDI-клавиатура»). Клавиатуру слышит
// panel.js (useMidi) — сюда ноты не приходят. Здесь — мелодии, прогресс, клавиатура и голос колонки.
//
// Мелодии в коде — только народные и классика, чьи авторы умерли больше 70 лет назад; ноты набраны по мелодии, без
// слов песен и чужих файлов. Свои мелодии человек добавляет файлом .mid — они в kv (melody:<id>), сам файл нигде не
// хранится. Меняете вид мелодий или прогресса — поднимите dataVersion в манифесте.
//
// Запись мелодии: «E4:1 D4:0.5 -:1» — нота (буква, ♯ как #, ♭ как b, октава; до первой октавы — C4) и длина в долях;
// «-» — пауза. Без длины — 1.
//
// Тексты для людей — словарём по языкам (docs/plugins/AUTHORING.md, «Языки»): host.i18n.t берёт их на языке дома,
// у мелодий названия — по языкам, host.i18n.pick.

const MELODIES = [
  {
    id: "oda", title: "Ода к радости", about: "Л. ван Бетховен, 1824", tempo: 110, level: "лёгкая",
    en: { title: "Ode to Joy", about: "L. van Beethoven, 1824", level: "easy" },
    tune: "E4 E4 F4 G4 G4 F4 E4 D4 C4 C4 D4 E4 E4:1.5 D4:0.5 D4:2 " +
          "E4 E4 F4 G4 G4 F4 E4 D4 C4 C4 D4 E4 D4:1.5 C4:0.5 C4:2",
  },
  {
    id: "mama", title: "Ах, скажу я вам, мама", about: "французская народная (Twinkle, Twinkle)", tempo: 100, level: "лёгкая",
    en: { title: "Twinkle, Twinkle, Little Star", about: "French folk tune", level: "easy" },
    tune: "C4 C4 G4 G4 A4 A4 G4:2 F4 F4 E4 E4 D4 D4 C4:2 G4 G4 F4 F4 E4 E4 D4:2 " +
          "G4 G4 F4 F4 E4 E4 D4:2 C4 C4 G4 G4 A4 A4 G4:2 F4 F4 E4 E4 D4 D4 C4:2",
  },
  {
    id: "yakov", title: "Братец Яков", about: "французская народная", tempo: 110, level: "лёгкая",
    en: { title: "Frère Jacques", about: "French folk song", level: "easy" },
    tune: "C4 D4 E4 C4 C4 D4 E4 C4 E4 F4 G4:2 E4 F4 G4:2 " +
          "G4:0.5 A4:0.5 G4:0.5 F4:0.5 E4 C4 G4:0.5 A4:0.5 G4:0.5 F4:0.5 E4 C4 C4 G3 C4:2 C4 G3 C4:2",
  },
  {
    id: "yolochka", title: "В лесу родилась ёлочка", about: "Л. Бекман, 1903; только мелодия", tempo: 120, level: "лёгкая",
    en: { title: "The Little Fir Tree", about: "L. Bekman, 1903; the tune only", level: "easy" },
    tune: "C4 A4 A4 G4 A4 F4 C4 C4 C4 A4 A4 Bb4 G4 C5:3 C5 D5 D5 Bb4 Bb4 A4 G4 F4 C4 A4 A4 G4 A4 F4:3",
  },
  {
    id: "bereza", title: "Во поле берёза стояла", about: "русская народная", tempo: 100, level: "лёгкая",
    en: { title: "The Birch Tree in the Field", about: "Russian folk song", level: "easy" },
    tune: "A4 A4 A4 A4 G4 F4 E4 E4 D4:2 A4 A4 C5 A4 A4 G4 F4 E4 E4 D4:2",
  },
  {
    id: "bubenchiki", title: "Бубенчики (Jingle Bells)", about: "Дж. Пирпонт, 1857", tempo: 120, level: "средняя",
    en: { title: "Jingle Bells", about: "J. Pierpont, 1857", level: "medium" },
    tune: "E4 E4 E4:2 E4 E4 E4:2 E4 G4 C4:1.5 D4:0.5 E4:4 F4 F4 F4:1.5 F4:0.5 F4 E4 E4 E4:0.5 E4:0.5 " +
          "E4 D4 D4 E4 D4:2 G4:2 E4 E4 E4:2 E4 E4 E4:2 E4 G4 C4:1.5 D4:0.5 E4:4 " +
          "F4 F4 F4:1.5 F4:0.5 F4 E4 E4 E4:0.5 E4:0.5 G4 G4 F4 D4 C4:4",
  },
  {
    id: "brahms", title: "Колыбельная", about: "И. Брамс, 1868", tempo: 90, level: "средняя",
    en: { title: "Lullaby", about: "J. Brahms, 1868", level: "medium" },
    tune: "E4:0.5 E4:0.5 G4:2 E4:0.5 E4:0.5 G4:2 E4:0.5 G4:0.5 C5 B4:1.5 A4:0.5 A4 G4 " +
          "D4:0.5 E4:0.5 F4 D4 D4:0.5 E4:0.5 F4:2 D4:0.5 F4:0.5 B4:0.5 A4:0.5 G4 B4 C5:2",
  },
  {
    id: "eliza", title: "К Элизе, начало", about: "Л. ван Бетховен, 1810", tempo: 120, level: "средняя",
    en: { title: "Für Elise, the opening", about: "L. van Beethoven, 1810", level: "medium" },
    tune: "E5:0.5 D#5:0.5 E5:0.5 D#5:0.5 E5:0.5 B4:0.5 D5:0.5 C5:0.5 A4:1.5 C4:0.5 E4:0.5 A4:0.5 B4:1.5 " +
          "E4:0.5 G#4:0.5 B4:0.5 C5:1.5 E4:0.5 E5:0.5 D#5:0.5 E5:0.5 D#5:0.5 E5:0.5 B4:0.5 D5:0.5 C5:0.5 " +
          "A4:1.5 C4:0.5 E4:0.5 A4:0.5 B4:1.5 E4:0.5 C5:0.5 B4:0.5 A4:2",
  },
];

// Слова учителя по языкам: язык дома выбирает host.i18n.t.
const T = {
  ru: {
    noMelody: "Такой мелодии нет.",
    noNotes: "В мелодии нет нот.",
    tooLong: "Мелодия длиннее 2000 нот.",
    ownTitle: "Своя мелодия",
    ownAbout: "своя, из файла",
    ownLevel: "своя",
    screen: "экран",
    notFound: "Такой мелодии нет. Есть: {list}.",
    learning: "Учим «{title}». Клавиатура ждёт в пульте.",
    learningNow: "Учим «{title}».",
    passed: " Пройдена {times} раз, лучший раз — ошибок {best}.",
    list: "Мелодии: {list}{more}.",
    more: " и другие",
  },
  en: {
    noMelody: "There is no such tune.",
    noNotes: "The tune has no notes.",
    tooLong: "The tune is longer than 2000 notes.",
    ownTitle: "My tune",
    ownAbout: "my own, from a file",
    ownLevel: "own",
    screen: "screen",
    notFound: "There is no such tune. There are: {list}.",
    learning: "Learning “{title}”. The keyboard is waiting in the panel.",
    learningNow: "Learning “{title}”.",
    passed: " Played {times} times, the best time — {best} mistakes.",
    list: "Tunes: {list}{more}.",
    more: " and others",
  },
};
const t = (key, params) => host.i18n.t(T, key, params);

const STEPS = { C: 0, D: 2, E: 4, F: 5, G: 7, A: 9, B: 11 };

// «E4:1 D4:0.5 -:1» → [[64, 0, 1], [62, 1, 0.5]] — нота MIDI, начало и длина в долях.
function tune(text) {
  const notes = [];
  let at = 0;
  for (const word of text.trim().split(/\s+/)) {
    const [name, length = "1"] = word.split(":");
    const beats = Number(length);
    const found = /^([A-G])([#b]?)(-?\d)$/.exec(name);
    if (found) {
      const note = 12 * (Number(found[3]) + 1) + STEPS[found[1]] + (found[2] === "#" ? 1 : found[2] === "b" ? -1 : 0);
      notes.push([note, at, beats]);
    }
    at += beats;
  }
  return notes;
}

const plain = (text) => String(text ?? "").toLowerCase().replace(/ё/g, "е").replace(/[^\p{L}\p{N}\s]+/gu, " ").replace(/\s+/g, " ").trim();

// Названия — на языке дома; русские остаются для поиска по словам («давай учить ёлочку»).
function builtIn() {
  return MELODIES.map(({ tune: text, en, ...melody }) => {
    const words = host.i18n.pick({ ru: { title: melody.title, about: melody.about, level: melody.level }, en });
    return { ...melody, ...words, own: false, notes: tune(text), aliases: [melody.title, en.title] };
  });
}

function own() {
  return host.kv.keys("melody:").map((key) => host.kv.get(key)).filter(Boolean).map((melody) => ({ ...melody, own: true }));
}

function all() {
  return [...builtIn(), ...own()];
}

// Мелодия по словам человека: «ёлочку», «оду к радости», «колыбельную».
function find(words) {
  const asked = plain(words);
  if (!asked) return null;
  const stem = (word) => word.slice(0, Math.max(3, word.length - 2));
  return (
    all().find((melody) => (melody.aliases ?? [melody.title]).some((name) => plain(name) === asked)) ??
    all().find((melody) => (melody.aliases ?? [melody.title]).map(plain).join(" ").split(" ").some((word) => word.length > 2 && asked.split(" ").some((part) => stem(part) === stem(word))))
  );
}

// ── Пульт ──

// Всё, что рисует учитель. Только читает: запись в kv — весть пульту, и все его части перечитали бы данные.
export function state() {
  const melodies = all();
  const current = host.kv.get("current") ?? melodies[0]?.id;
  const progress = {};
  for (const melody of melodies) progress[melody.id] = host.kv.get(`progress:${melody.id}`) ?? null;
  const keyboards = {};
  for (const key of host.kv.keys("keyboard:")) keyboards[key.slice("keyboard:".length)] = host.kv.get(key);
  return { melodies, current, progress, keyboards, names: host.settings.get("names") !== false };
}

export function choose({ id }) {
  if (!all().some((melody) => melody.id === id)) throw new Error(t("noMelody"));
  host.kv.set("current", id);
  return { current: id };
}

// Своя мелодия из файла .mid: название, темп и ноты [нота, начало, длина] в долях.
export function add({ title, tempo, notes }) {
  if (!Array.isArray(notes) || notes.length === 0) throw new Error(t("noNotes"));
  if (notes.length > 2000) throw new Error(t("tooLong"));
  const clean = notes
    .filter((note) => Array.isArray(note) && note.length >= 3)
    .map(([note, at, length]) => [Math.round(note), Math.round(at * 1000) / 1000, Math.round(length * 1000) / 1000]);
  const name = String(title ?? "").trim().slice(0, 60) || t("ownTitle");
  const id = `own-${Date.now().toString(36)}`;
  host.kv.set(`melody:${id}`, { id, title: name, about: t("ownAbout"), tempo: Number(tempo) || 100, level: t("ownLevel"), notes: clean });
  host.kv.set("current", id);
  return { id };
}

export function remove({ id }) {
  host.kv.delete(`melody:${id}`);
  host.kv.delete(`progress:${id}`);
  if (host.kv.get("current") === id) host.kv.delete("current");
  return { removed: id };
}

// Клавиатура: сколько клавиш, самая левая нота, звучит ли сама. Хранится по имени — у каждой своя; `reset` — забыть,
// чтобы спросить заново.
export function keyboard({ name, keys, low, sounds, reset }) {
  const key = `keyboard:${String(name ?? t("screen")).slice(0, 60)}`;
  if (reset === true) {
    host.kv.delete(key);
    return {};
  }
  const was = host.kv.get(key) ?? {};
  const next = { ...was };
  if ([25, 32, 37, 49, 61, 76, 88].includes(Number(keys))) next.keys = Number(keys);
  if (Number.isInteger(low) && low >= 0 && low <= 120) next.low = low;
  if (typeof sounds === "boolean") next.sounds = sounds;
  host.kv.set(key, next);
  return next;
}

// Мелодию прошли до конца: сколько раз и лучший проход (меньше ошибок).
export function done({ id, mistakes }) {
  const was = host.kv.get(`progress:${id}`) ?? { times: 0, best: null };
  const best = was.best == null ? mistakes : Math.min(was.best, mistakes);
  const next = { times: was.times + 1, best, last: mistakes };
  host.kv.set(`progress:${id}`, next);
  return next;
}

// ── Колонка ──

// «Давай учить ёлочку», “let's learn Jingle Bells”.
export function learn({ melody }) {
  const found = find(melody);
  if (!found) return t("notFound", { list: all().slice(0, 5).map((m) => m.title).join(", ") });
  host.kv.set("current", found.id);
  return t("learning", { title: found.title });
}

// «Какие мелодии есть», «что мы учим»; “what tunes are there”, “what are we learning”.
export function about({ request }) {
  const text = plain(request?.text);
  const melodies = all();
  if (text.includes("учим") || text.includes("learning")) {
    const current = melodies.find((melody) => melody.id === host.kv.get("current")) ?? melodies[0];
    const progress = host.kv.get(`progress:${current.id}`);
    const times = progress ? t("passed", { times: progress.times, best: progress.best }) : "";
    return t("learningNow", { title: current.title }) + times;
  }
  return t("list", { list: melodies.slice(0, 5).map((melody) => melody.title).join(", "), more: melodies.length > 5 ? t("more") : "" });
}
