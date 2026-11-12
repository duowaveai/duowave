// __TITLE__: учитель игры на MIDI-клавиатуре (docs/plugins/AUTHORING.md, «MIDI-клавиатура»). Клавиатуру слышит
// panel.js (useMidi) — сюда ноты не приходят. Здесь — мелодии, прогресс, клавиатура и голос колонки.
//
// Мелодии в коде — только народные и классика, чьи авторы умерли больше 70 лет назад; ноты набраны по мелодии, без
// слов песен и чужих файлов. Свои мелодии человек добавляет файлом .mid — они в kv (melody:<id>), сам файл нигде не
// хранится. Меняете вид мелодий или прогресса — поднимите dataVersion в манифесте.
//
// Запись мелодии: «E4:1 D4:0.5 -:1» — нота (буква, ♯ как #, ♭ как b, октава; до первой октавы — C4) и длина в долях;
// «-» — пауза. Без длины — 1.

const MELODIES = [
  {
    id: "oda", title: "Ода к радости", about: "Л. ван Бетховен, 1824", tempo: 110, level: "лёгкая",
    tune: "E4 E4 F4 G4 G4 F4 E4 D4 C4 C4 D4 E4 E4:1.5 D4:0.5 D4:2 " +
          "E4 E4 F4 G4 G4 F4 E4 D4 C4 C4 D4 E4 D4:1.5 C4:0.5 C4:2",
  },
  {
    id: "mama", title: "Ах, скажу я вам, мама", about: "французская народная (Twinkle, Twinkle)", tempo: 100, level: "лёгкая",
    tune: "C4 C4 G4 G4 A4 A4 G4:2 F4 F4 E4 E4 D4 D4 C4:2 G4 G4 F4 F4 E4 E4 D4:2 " +
          "G4 G4 F4 F4 E4 E4 D4:2 C4 C4 G4 G4 A4 A4 G4:2 F4 F4 E4 E4 D4 D4 C4:2",
  },
  {
    id: "yakov", title: "Братец Яков", about: "французская народная", tempo: 110, level: "лёгкая",
    tune: "C4 D4 E4 C4 C4 D4 E4 C4 E4 F4 G4:2 E4 F4 G4:2 " +
          "G4:0.5 A4:0.5 G4:0.5 F4:0.5 E4 C4 G4:0.5 A4:0.5 G4:0.5 F4:0.5 E4 C4 C4 G3 C4:2 C4 G3 C4:2",
  },
  {
    id: "yolochka", title: "В лесу родилась ёлочка", about: "Л. Бекман, 1903; только мелодия", tempo: 120, level: "лёгкая",
    tune: "C4 A4 A4 G4 A4 F4 C4 C4 C4 A4 A4 Bb4 G4 C5:3 C5 D5 D5 Bb4 Bb4 A4 G4 F4 C4 A4 A4 G4 A4 F4:3",
  },
  {
    id: "bereza", title: "Во поле берёза стояла", about: "русская народная", tempo: 100, level: "лёгкая",
    tune: "A4 A4 A4 A4 G4 F4 E4 E4 D4:2 A4 A4 C5 A4 A4 G4 F4 E4 E4 D4:2",
  },
  {
    id: "bubenchiki", title: "Бубенчики (Jingle Bells)", about: "Дж. Пирпонт, 1857", tempo: 120, level: "средняя",
    tune: "E4 E4 E4:2 E4 E4 E4:2 E4 G4 C4:1.5 D4:0.5 E4:4 F4 F4 F4:1.5 F4:0.5 F4 E4 E4 E4:0.5 E4:0.5 " +
          "E4 D4 D4 E4 D4:2 G4:2 E4 E4 E4:2 E4 E4 E4:2 E4 G4 C4:1.5 D4:0.5 E4:4 " +
          "F4 F4 F4:1.5 F4:0.5 F4 E4 E4 E4:0.5 E4:0.5 G4 G4 F4 D4 C4:4",
  },
  {
    id: "brahms", title: "Колыбельная", about: "И. Брамс, 1868", tempo: 90, level: "средняя",
    tune: "E4:0.5 E4:0.5 G4:2 E4:0.5 E4:0.5 G4:2 E4:0.5 G4:0.5 C5 B4:1.5 A4:0.5 A4 G4 " +
          "D4:0.5 E4:0.5 F4 D4 D4:0.5 E4:0.5 F4:2 D4:0.5 F4:0.5 B4:0.5 A4:0.5 G4 B4 C5:2",
  },
  {
    id: "eliza", title: "К Элизе, начало", about: "Л. ван Бетховен, 1810", tempo: 120, level: "средняя",
    tune: "E5:0.5 D#5:0.5 E5:0.5 D#5:0.5 E5:0.5 B4:0.5 D5:0.5 C5:0.5 A4:1.5 C4:0.5 E4:0.5 A4:0.5 B4:1.5 " +
          "E4:0.5 G#4:0.5 B4:0.5 C5:1.5 E4:0.5 E5:0.5 D#5:0.5 E5:0.5 D#5:0.5 E5:0.5 B4:0.5 D5:0.5 C5:0.5 " +
          "A4:1.5 C4:0.5 E4:0.5 A4:0.5 B4:1.5 E4:0.5 C5:0.5 B4:0.5 A4:2",
  },
];

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

function builtIn() {
  return MELODIES.map(({ tune: text, ...melody }) => ({ ...melody, own: false, notes: tune(text) }));
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
    all().find((melody) => plain(melody.title) === asked) ??
    all().find((melody) => plain(melody.title).split(" ").some((word) => word.length > 2 && asked.split(" ").some((part) => stem(part) === stem(word))))
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
  if (!all().some((melody) => melody.id === id)) throw new Error("Такой мелодии нет.");
  host.kv.set("current", id);
  return { current: id };
}

// Своя мелодия из файла .mid: название, темп и ноты [нота, начало, длина] в долях.
export function add({ title, tempo, notes }) {
  if (!Array.isArray(notes) || notes.length === 0) throw new Error("В мелодии нет нот.");
  if (notes.length > 2000) throw new Error("Мелодия длиннее 2000 нот.");
  const clean = notes
    .filter((note) => Array.isArray(note) && note.length >= 3)
    .map(([note, at, length]) => [Math.round(note), Math.round(at * 1000) / 1000, Math.round(length * 1000) / 1000]);
  const name = String(title ?? "").trim().slice(0, 60) || "Своя мелодия";
  const id = `own-${Date.now().toString(36)}`;
  host.kv.set(`melody:${id}`, { id, title: name, about: "своя, из файла", tempo: Number(tempo) || 100, level: "своя", notes: clean });
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
  const key = `keyboard:${String(name ?? "экран").slice(0, 60)}`;
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

// «Давай учить ёлочку».
export function learn({ melody }) {
  const found = find(melody);
  if (!found) return `Такой мелодии нет. Есть: ${all().slice(0, 5).map((m) => m.title).join(", ")}.`;
  host.kv.set("current", found.id);
  return `Учим «${found.title}». Клавиатура ждёт в пульте.`;
}

// «Какие мелодии есть», «что мы учим».
export function about({ request }) {
  const text = plain(request?.text);
  const melodies = all();
  if (text.includes("учим")) {
    const current = melodies.find((melody) => melody.id === host.kv.get("current")) ?? melodies[0];
    const progress = host.kv.get(`progress:${current.id}`);
    const times = progress ? ` Пройдена ${progress.times} раз, лучший раз — ошибок ${progress.best}.` : "";
    return `Учим «${current.title}».${times}`;
  }
  return `Мелодии: ${melodies.slice(0, 5).map((melody) => melody.title).join(", ")}${melodies.length > 5 ? " и другие" : ""}.`;
}
