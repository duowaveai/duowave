// «Папки» — голосом по домашней папке DuoWave (docs/plans/shared-folders.md). «Что в папке Фото», «открой папку
// Рецепты», «покажи фото из Отпуска», «прочитай файл борщ», «найди рецепт борща», «запиши в папку Рецепты блины…»,
// «что на флешке». Дальше разговор идёт без «Алиса» три минуты: «дальше», «предыдущая», «назад», «номер три»,
// «открой …», «прочитай», «найди …», «запиши …», «хватит».
//
// Фото и видео открываются и окном на экране компьютера (device/screen): «оставь», «закрой», «пауза», «продолжи»;
// агенту — folder_show и folder_play, окнами он правит инструментами screen_* шлюза.
//
// Где человек сейчас — в своём kv по колонке (`walk:<колонка>`), последняя колонка — `last`: большая карточка на столе
// показывает то же место, её кнопки ведут ту же прогулку. Код ничего не решает о том, что уходит из дома: шлюз говорит
// `speak` у папки (можно ли называть файлы вслух — свой голос или галочка), а облачной модели без галочки папка сама не
// даётся. Без `speak` «Папки» говорят только числа и «на экране».

const HOME = "files/home";
const ATTACHED = "files/home/Подключено";
const PAGE = 200;
const SAID = 5;
const CHUNK = 900;
const WALK = { handler: "walk" };
const SCREEN = "device/screen";
const SHOWN = ["image", "video", "audio"];

// ——— Слова ———

function norm(text) {
  return String(text ?? "")
    .toLowerCase()
    .replace(/ё/g, "е")
    .replace(/[«»"'.,!?:;()_]/g, " ")
    .replace(/[-–—]/g, " ")
    .replace(/\s+/g, " ")
    .trim();
}

// Имя без расширения: «Борщ.txt» → «Борщ».
function stem(name) {
  const text = String(name ?? "");
  const dot = text.lastIndexOf(".");
  return dot > 0 && text.length - dot <= 6 ? text.slice(0, dot) : text;
}

function distance(a, b) {
  const row = Array.from({ length: b.length + 1 }, (_, index) => index);
  for (let i = 1; i <= a.length; i += 1) {
    let previous = row[0];
    row[0] = i;
    for (let j = 1; j <= b.length; j += 1) {
      const kept = row[j];
      row[j] = Math.min(row[j] + 1, row[j - 1] + 1, previous + (a[i - 1] === b[j - 1] ? 0 : 1));
      previous = kept;
    }
  }
  return row[b.length];
}

// Кириллица латиницей: «скриншот» — «skrinshot», рядом с «Screenshot» в имени файла.
const LATIN = {
  а: "a", б: "b", в: "v", г: "g", д: "d", е: "e", ж: "zh", з: "z", и: "i", й: "y", к: "k", л: "l", м: "m", н: "n",
  о: "o", п: "p", р: "r", с: "s", т: "t", у: "u", ф: "f", х: "h", ц: "ts", ч: "ch", ш: "sh", щ: "sch", ъ: "", ы: "y",
  ь: "", э: "e", ю: "yu", я: "ya",
};

function latin(text) {
  return [...text].map((letter) => LATIN[letter] ?? letter).join("");
}

function likenessOf(a, b) {
  if (!a || !b) return 0;
  if (a === b) return 1;
  if (b.startsWith(`${a} `) || a.startsWith(`${b} `)) return 0.9;
  const shortest = Math.min(a.length, b.length);
  if (shortest >= 4 && a.slice(0, shortest - 2) === b.slice(0, shortest - 2)) return 0.85;
  const words = b.split(" ");
  if (a.length >= 4 && words.some((word) => word.length >= 4 && (word.startsWith(a.slice(0, -1)) || a.startsWith(word.slice(0, -1))))) {
    return 0.75;
  }
  let score = 1 - distance(a, b) / Math.max(a.length, b.length);
  // Одно слово — на похожее слово имени: «скриншот» на «Screenshot 2026-11-13 at 16.01.11».
  if (a.length >= 5 && !a.includes(" ")) {
    for (const word of words) {
      if (word.length >= 5) score = Math.max(score, 1 - distance(a, word) / Math.max(a.length, word.length));
    }
  }
  return score;
}

// Насколько сказанное похоже на имя: «отпуска» — на «Отпуск», «борщ» — на «Борщ украинский», «скриншот» — на
// «Screenshot…» (сказанное по-русски сравнивается и латиницей).
function likeness(said, name) {
  const a = norm(said);
  const b = norm(stem(name));
  const direct = likenessOf(a, b);
  const crossed = /[а-я]/.test(a + b) && /[a-z]/.test(a + b) ? likenessOf(latin(a), latin(b)) : 0;
  return Math.max(direct, crossed);
}

function scored(said, entries) {
  let top = null;
  for (const entry of entries) {
    const score = likeness(said, entry.name);
    if (score > 0 && (!top || score > top.score)) top = { entry, score };
  }
  return top;
}

function best(said, entries, at = 0.6) {
  const top = scored(said, entries);
  return top && top.score >= at ? top.entry : null;
}

const ORDINALS = {
  первый: 1, первая: 1, первую: 1, первое: 1, второй: 2, вторая: 2, вторую: 2, второе: 2, третий: 3, третья: 3,
  третью: 3, третье: 3, четвертый: 4, четвертая: 4, четвертую: 4, пятый: 5, пятая: 5, пятую: 5, шестой: 6, шестую: 6,
  седьмой: 7, седьмую: 7, восьмой: 8, восьмую: 8, девятый: 9, девятую: 9, десятый: 10, десятую: 10,
};
const NUMBERS = { один: 1, одна: 1, два: 2, две: 2, три: 3, четыре: 4, пять: 5, шесть: 6, семь: 7, восемь: 8, девять: 9, десять: 10 };

function numberIn(text) {
  const words = norm(text).split(" ");
  for (const word of words) {
    if (/^\d+$/.test(word)) return Number(word);
    if (ORDINALS[word]) return ORDINALS[word];
    if (NUMBERS[word]) return NUMBERS[word];
  }
  return null;
}

function plural(count, one, few, many) {
  const tens = count % 100;
  const units = count % 10;
  if (tens >= 11 && tens <= 14) return many;
  if (units === 1) return one;
  if (units >= 2 && units <= 4) return few;
  return many;
}

function counted(folders, files) {
  const parts = [];
  if (folders) parts.push(`${folders} ${plural(folders, "папка", "папки", "папок")}`);
  if (files) parts.push(`${files} ${plural(files, "файл", "файла", "файлов")}`);
  return parts.join(" и ") || "ничего";
}

const nameOf = (address) => String(address ?? "").split("/").pop();

// Папка-участник, в которой лежит адрес: files/home/Фото или files/home/Подключено/Флешка.
function folderOf(address) {
  const parts = String(address ?? "").split("/");
  if (parts[0] !== "files") return address;
  if (parts[1] === "home" && parts[2] === "Подключено" && parts.length > 3) return parts.slice(0, 4).join("/");
  return parts.slice(0, 3).join("/");
}

function capital(text) {
  const value = String(text ?? "").trim();
  return value ? value[0].toUpperCase() + value.slice(1) : value;
}

// ——— Где человек ———

function deviceOf(request) {
  return String(request?.device ?? "panel");
}

function walkOf(device) {
  return host.kv.get(`walk:${device}`) ?? null;
}

function keep(device, walk) {
  host.kv.set(`walk:${device}`, walk);
  host.kv.set("last", device);
}

// ——— Экран ———

// Окна на экране компьютера (device/screen, gateway/src/screen): фото, видео, звук — каждое своим окном поверх всех.
// Ссылку на файл даёт шлюз (`media` папки), экран показывает только её. Без `add` новые окна встают на место временных;
// оставленные («оставь») стоят, пока их не закроют. Экрана нет (шлюз на колонке, нет приложения DuoWave) — бросает.
async function onScreen(addresses, add = false) {
  const items = [];
  for (const address of addresses) {
    const media = await host.bus.call(address, "media", {});
    items.push({ kind: media.kind, title: media.title, file: media.file, from: address });
  }
  const answer = await host.bus.call(SCREEN, "open", { items, add });
  return answer.opened ?? [];
}

// То же без экрана — null: тогда показывает только карточка на столе.
async function screenTry(addresses, add = false) {
  try {
    return await onScreen(addresses, add);
  } catch {
    return null;
  }
}

// Окнам: оставить, закрыть, пауза, продолжить; не вышло (окна уже нет, экрана нет) — null.
async function toScreen(handle, input = {}) {
  try {
    return await host.bus.call(SCREEN, handle, input);
  } catch {
    return null;
  }
}

// ——— Папка ———

// Папки дома, которые приложению видны (его связи на шине): имя для голоса — заголовок участника.
async function known() {
  const seen = await host.bus.list();
  return seen
    .filter((participant) => participant.address.startsWith(`${HOME}/`) && participant.address !== ATTACHED)
    .map((participant) => ({ address: participant.address, name: participant.title, folder: true, kind: "folder" }));
}

// Можно ли называть файлы этой папки вслух (шлюз считает: свой голос или галочка «Голос может называть файлы»).
async function speakable(address) {
  try {
    const stat = await host.files.stat(folderOf(address));
    return stat.speak === true;
  } catch {
    return false;
  }
}

async function listing(path) {
  return host.files.list(path, { limit: PAGE });
}

function crumbs(path) {
  const parts = String(path ?? "").split("/").slice(2);
  return parts.filter((part) => part !== "Подключено" || parts.length === 1).join(" › ");
}

// Список вслух: до пяти имён и «и ещё», или — без `speak` — только числа.
function sayList(path, page, speak, from = 0) {
  const title = nameOf(path);
  if (page.total === 0) return `В «${title}» пусто.`;
  const folders = page.items.filter((item) => item.folder).length;
  const counts = counted(folders, page.total - folders);
  if (!speak) return `В «${title}» ${counts}. Они на экране.`;
  const said = page.items.slice(from, from + SAID).map((item) => stem(item.name));
  const rest = page.total - from - said.length;
  const head = from === 0 ? `В «${title}» ${counts}: ` : "Дальше: ";
  return `${head}${said.join(", ")}${rest > 0 ? `, и ещё ${rest}` : ""}. Что открыть?`;
}

async function goTo(device, path) {
  const page = await listing(path);
  const speak = await speakable(path);
  keep(device, { path, items: page.items, total: page.total, said: SAID, open: null, kind: null, part: 0, offset: 0 });
  return { say: sayList(path, page, speak), continue: WALK };
}

// Открыть строку списка: папку — зайти, картинку — показать, документ — прочитать, звук — сыграть.
async function openItem(device, walk, item) {
  if (item.folder) return goTo(device, item.address);
  const speak = await speakable(item.address);
  const index = walk.items.findIndex((entry) => entry.address === item.address);
  const shown = { ...walk, open: item.address, kind: item.kind, index, part: 0, offset: 0 };
  if (item.kind === "image") {
    keep(device, shown);
    await screenTry([item.address]);
    return { say: speak ? `Показываю «${stem(item.name)}».` : "Показываю на экране.", continue: WALK };
  }
  if (item.kind === "video") {
    keep(device, shown);
    const opened = await screenTry([item.address]);
    if (!opened) return { say: "Видео показываю окном на компьютере с приложением DuoWave, а его здесь нет.", continue: WALK };
    return { say: speak ? `Включаю «${stem(item.name)}» на экране.` : "Включаю на экране.", continue: WALK };
  }
  if (item.kind === "audio") {
    const element = await host.files.play(item.address);
    keep(device, shown);
    return { say: speak ? `Включаю «${stem(item.name)}».` : "Включаю.", play: [element] };
  }
  if (["text", "pdf", "doc"].includes(item.kind)) {
    keep(device, shown);
    return readOn(device, shown, speak);
  }
  keep(device, shown);
  return { say: "Этот файл на экране: прочитать его вслух я не могу.", continue: WALK };
}

// Следующий кусок текста: по ~900 знаков, на границе предложения.
async function readOn(device, walk, speak) {
  let { part = 0, offset = 0 } = walk;
  let text;
  try {
    text = await host.files.text(walk.open, { part });
  } catch (error) {
    return { say: `Не читается: ${String(error?.message ?? error).slice(0, 200)}`, continue: WALK };
  }
  let rest = text.text.slice(offset);
  if (!rest.trim()) {
    if (part + 1 >= text.parts) return { say: "Это всё.", continue: WALK };
    part += 1;
    offset = 0;
    text = await host.files.text(walk.open, { part });
    rest = text.text;
  }
  let piece = rest.slice(0, CHUNK);
  if (rest.length > CHUNK) {
    const cut = Math.max(piece.lastIndexOf(". "), piece.lastIndexOf("\n"));
    if (cut > CHUNK / 2) piece = piece.slice(0, cut + 1);
  }
  keep(device, { ...walk, part, offset: offset + piece.length, reading: piece });
  if (!speak) return { say: "Текст на экране.", continue: WALK };
  const more = offset + piece.length < text.text.length || part + 1 < text.parts;
  return { say: `${piece.trim()}${more ? " … Дальше?" : ""}`.slice(0, 1000), continue: WALK };
}

// Соседняя вещь того же рода: «следующая» фотография, «предыдущий» документ.
async function step(device, walk, by) {
  if (!walk.open) {
    if (by > 0 && walk.said < walk.total) {
      const page = { items: walk.items, total: walk.total };
      const speak = await speakable(walk.path);
      keep(device, { ...walk, said: walk.said + SAID });
      return { say: speak ? sayList(walk.path, page, true, walk.said) : "Остальное на экране.", continue: WALK };
    }
    return { say: by > 0 ? "Больше ничего. Скажите название или «назад»." : "Это начало списка.", continue: WALK };
  }
  const same = walk.items.filter((item) => !item.folder && (item.kind === walk.kind || walk.kind === null));
  const at = same.findIndex((item) => item.address === walk.open);
  const next = same[at + by];
  if (!next) return { say: by > 0 ? "Это последняя." : "Это первая.", continue: WALK };
  return openItem(device, walk, next);
}

// ——— Голос ———

export async function folders({ request }) {
  const list = await known();
  const device = deviceOf(request);
  keep(device, { path: null, items: list, total: list.length, said: list.length, open: null, kind: null });
  if (list.length === 0) return "Папок пока нет.";
  return { say: `Папки: ${list.map((folder) => folder.name).join(", ")}. Какую открыть?`, continue: WALK };
}

async function folderNamed(name) {
  const list = await known();
  return best(name, list);
}

export async function openFolder({ name, request }) {
  const folder = await folderNamed(name);
  if (!folder) return `Папки «${name}» нет. Скажите «какие у меня папки».`;
  return goTo(deviceOf(request), folder.address);
}

// «Покажи фото из отпуска»: папка — среди папок дома, а не нашлась — среди вложенных; первая картинка на экране.
export async function photos({ name, request }) {
  const device = deviceOf(request);
  let folder = await folderNamed(name);
  if (!folder) {
    for (const home of await known()) {
      const found = await host.files.find(home.address, name, { limit: 5 }).catch(() => ({ items: [] }));
      folder = found.items.find((item) => item.folder) ?? null;
      if (folder) break;
    }
  }
  if (!folder) return `Папки «${name}» не нашла.`;
  const page = await listing(folder.address);
  keep(device, { path: folder.address, items: page.items, total: page.total, said: SAID, open: null, kind: "image" });
  const first = page.items.find((item) => item.kind === "image");
  if (!first) return { say: `В «${nameOf(folder.address)}» нет фотографий.`, continue: WALK };
  const pictures = page.items.filter((item) => item.kind === "image").length;
  const answer = await openItem(device, walkOf(device), first);
  return { ...answer, say: `В «${nameOf(folder.address)}» ${pictures} ${plural(pictures, "фото", "фото", "фото")}. Показываю первое.` };
}

// Ищет по всем папкам дома (и по укороченному слову: «борща» — «борщ»).
async function findEverywhere(query, within) {
  const roots = within ? [{ address: within }] : await known();
  const hits = [];
  const words = [query];
  const short = norm(query).replace(/[аеиоуыэюяйь]+$/u, "");
  if (short.length >= 3 && short !== norm(query)) words.push(short);
  for (const root of roots) {
    for (const word of words) {
      const found = await host.files.find(root.address, word, { limit: 10 }).catch(() => ({ items: [] }));
      for (const item of found.items) if (!hits.some((hit) => hit.address === item.address)) hits.push(item);
    }
  }
  hits.sort((a, b) => b.score - a.score);
  return hits;
}

export async function find({ query, request }) {
  const device = deviceOf(request);
  const hits = await findEverywhere(query);
  if (hits.length === 0) return `Ничего не нашла по «${query}».`;
  const top = hits[0];
  const path = top.address.split("/").slice(0, -1).join("/");
  const page = await listing(path);
  const walk = { path, items: page.items, total: page.total, said: SAID, open: null, kind: null };
  keep(device, walk);
  const answer = await openItem(device, walk, page.items.find((item) => item.address === top.address) ?? top);
  if (hits.length > 1 && answer.say && (await speakable(top.address))) {
    return { ...answer, say: `Нашла «${stem(top.name)}». ${answer.say}`.slice(0, 1000) };
  }
  return answer;
}

export async function readFile({ name, request }) {
  const device = deviceOf(request);
  const walk = walkOf(device);
  const here = walk?.items ? best(name, walk.items.filter((item) => !item.folder)) : null;
  if (here) return openItem(device, walk, here);
  return find({ query: name, request });
}

// «Запиши в папку Рецепты блины два яйца стакан муки»: папка — первые слова, имя файла — следующее слово.
export async function write({ rest, request }) {
  const words = String(rest ?? "").trim().split(/\s+/);
  const list = await known();
  // The folder's name is the first one to three words: the take that is most like a folder wins («мои рецепты»).
  let chosen = null;
  for (let take = 1; take <= Math.min(3, words.length - 1); take += 1) {
    const said = words.slice(0, take).join(" ");
    for (const folder of list) {
      const score = likeness(said, folder.name);
      if (score >= 0.75 && (!chosen || score > chosen.score)) chosen = { folder, take, score };
    }
  }
  if (chosen) return writeInto(deviceOf(request), chosen.folder.address, words.slice(chosen.take).join(" "));
  return "В какую папку? Скажите: «запиши в папку Рецепты …».";
}

async function writeInto(device, folder, said) {
  const text = String(said ?? "").trim();
  if (!text) return { say: "Что записать?", continue: WALK };
  const parts = text.match(/^(.+?)\s*[:—–-]\s+(.+)$/s);
  const name = capital(parts ? parts[1] : text.split(/\s+/)[0]).slice(0, 60);
  const body = parts ? capital(parts[2]) : capital(text);
  let address = `${folder}/${name}.txt`;
  for (let n = 2; n < 50; n += 1) {
    const there = await host.files.stat(address).then(() => true).catch(() => false);
    if (!there) break;
    address = `${folder}/${name} ${n}.txt`;
  }
  await host.files.write(address, `${body}\n`);
  const page = await listing(folder);
  keep(device, { path: folder, items: page.items, total: page.total, said: SAID, open: address, kind: "text", part: 0, offset: 0 });
  return { say: `Записала «${nameOf(address).replace(/\.txt$/, "")}» в «${nameOf(folder)}».`, continue: WALK };
}

export async function drive({ request }) {
  const attached = await host.files.list(ATTACHED).catch(() => ({ items: [] }));
  const here = attached.items.filter((item) => item.here !== false);
  if (here.length === 0) {
    const offers = await host.bus.call(ATTACHED, "offers", {}).catch(() => ({ offers: [] }));
    if ((offers.offers ?? []).some((offer) => offer.kind === "drive")) {
      return { say: "Флешка подключена, но ещё не открыта. Открыть?", continue: { handler: "driveAnswer" } };
    }
    return "Флешек не вижу.";
  }
  return goTo(deviceOf(request), here[here.length - 1].address);
}

// Флешку воткнули, колонка свободна: шлюз зовёт это сам (плитка на столе спрашивает тоже).
export function driveFound({ name, offer, request }) {
  host.kv.set(`offer:${deviceOf(request)}`, offer);
  return { say: `Подключена ${String(name ?? "флешка").toLowerCase()}. Посмотреть, что на ней?`, continue: { handler: "driveAnswer" } };
}

const YES = /^(да|давай|ага|конечно|посмотреть|покажи|открой|хочу|можно)/;
const NO = /^(нет|не надо|не нужно|потом|отмена|не сейчас)/;

export async function driveAnswer({ text, request }) {
  const device = deviceOf(request);
  const said = norm(text);
  const offer = host.kv.get(`offer:${device}`) ?? undefined;
  if (NO.test(said)) {
    await host.bus.call(ATTACHED, "skip", offer ? { offer } : {}).catch(() => null);
    host.kv.delete(`offer:${device}`);
    return "Хорошо, не открываю.";
  }
  if (!YES.test(said)) return { say: "Открыть флешку? Скажите «да» или «нет».", continue: { handler: "driveAnswer" } };
  host.kv.delete(`offer:${device}`);
  try {
    const opened = await host.bus.call(ATTACHED, "look", offer ? { offer } : {});
    const answer = await goTo(device, opened.address);
    return { ...answer, say: `Открыла. ${answer.say}`.slice(0, 1000) };
  } catch (error) {
    return `Не открылась: ${String(error?.message ?? error).slice(0, 200)}`;
  }
}

// «Открой все»: фото и видео папки — окнами (до восьми, на месте временных), звук — на колонку очередью.
async function openAll(state, request) {
  const media = (state.items ?? []).filter((item) => SHOWN.includes(item.kind));
  if (media.length === 0) return { say: "Здесь нет фото, видео и звука.", continue: WALK };
  const pictures = media.filter((item) => item.kind !== "audio").slice(0, 8);
  const sounds = media.filter((item) => item.kind === "audio").slice(0, 20);
  const opened = pictures.length ? await screenTry(pictures.map((item) => item.address)) : [];
  const play = [];
  for (const sound of sounds) play.push(await host.files.play(sound.address));
  const parts = [];
  if (opened?.length) parts.push(`открыла ${opened.length} ${plural(opened.length, "окно", "окна", "окон")}`);
  if (pictures.length && !opened) parts.push("окнами показываю только на компьютере с приложением DuoWave");
  if (play.length) parts.push(`включаю ${play.length} ${plural(play.length, "запись", "записи", "записей")}`);
  const say = capital(`${parts.join(", ")}.`);
  keep(deviceOf(request), { ...state, open: null, kind: null });
  if (play.length) return { say, play };
  return { say, continue: WALK };
}

// Что человек назвал: в открытой папке или среди папок дома («открой документы», стоя в «Фото»); поровну — здешнее.
async function namedThing(state, said, foldersOnly = false) {
  const here = scored(said, state.items.filter((item) => !foldersOnly || item.folder));
  const home = scored(said, await known());
  const top = home && (!here || home.score > here.score) ? home : here;
  return top && top.score >= 0.6 ? top.entry : null;
}

// Фразы самого приложения: пока идёт разговор, всё сказанное приходит в walk, а не в них (живое 14.11: «что в папке
// фото» в разговоре было «Не поняла»).
const LIST_ASK = /^(какие (у меня |есть )?папки|мои папки|что в домашней папке)/;
const FOLDER_ASK = /^(?:что (?:лежит )?в папке|открой папку|покажи папку|зайди в папку|перейди в папку)\s+(.+)$/;
const PHOTOS_ASK = /^покажи (?:фото|фотографии|картинки|снимки) из\s+(.+)$/;
const DRIVE_ASK = /^(что на флешке|открой флешку|покажи флешку|что на диске)/;

// Слова в разговоре: «дальше», «назад», «номер три», «открой отпуск», «прочитай», «найди…», «запиши…», «хватит»,
// и фразы приложения: «что в папке документы», «покажи фото из отпуска», «какие у меня папки», «что на флешке».
export async function walk({ text, request }) {
  const device = deviceOf(request);
  const state = walkOf(device);
  const said = norm(text);
  if (!state) return folders({ request });
  // «Закрой видео», «закрой отпуск» — те окна; просто «закрой», «хватит» — временные окна, и разговор кончен. «Stop»
  // латиницей — так его иногда слышит распознавание.
  const closing = said.match(/^(?:закрой|закрыть|убери)(?:\s+(?:окно|окна))?\s*(.*)$/);
  if (closing && closing[1]) {
    const closed = await toScreen("close", { window: closing[1] });
    return { say: closed?.closed?.length ? "Закрыла." : `Окна «${closing[1]}» нет.`, continue: WALK };
  }
  // «Всё» — конец, только само по себе: «все три открой» — открыть все (живое 14.11).
  if (closing || /^(хватит|стоп|stop|спасибо|пока|отмена|выход)(\s|$)/.test(said) || /^(все|всё)( спасибо)?$/.test(said)) {
    await toScreen("close", { window: "temporary" });
    return "Хорошо.";
  }
  if (/^((открой|покажи|включи) (все|всё)|(все|всё)( \S+)? (открой|покажи|включи))/.test(said)) {
    return openAll(state, request);
  }
  // «Оставь» — окно того, что открыто сейчас, остаётся, пока его не закроют.
  if (/^(оставь|не закрывай|пусть останется)/.test(said)) {
    const kept = await toScreen("keep", { window: state.open ?? "temporary" });
    return { say: kept?.kept?.length ? "Оставила на экране." : "На экране нечего оставлять.", continue: WALK };
  }
  if (/^(пауза|на паузу|поставь на паузу|останови)/.test(said)) {
    const paused = await toScreen("pause", {});
    return { say: paused ? "Пауза." : "Ничего не играет.", continue: WALK };
  }
  if (/^(продолжи|продолжай|играй|включи снова)/.test(said)) {
    const resumed = await toScreen("resume", {});
    return { say: resumed ? "Продолжаю." : "Ничего не на паузе.", continue: WALK };
  }
  if (LIST_ASK.test(said)) return folders({ request });
  if (DRIVE_ASK.test(said)) return drive({ request });
  const photosAsked = said.match(PHOTOS_ASK);
  if (photosAsked) return photos({ name: photosAsked[1], request });
  const folderAsked = said.match(FOLDER_ASK);
  if (folderAsked) {
    const folder = await namedThing(state, folderAsked[1], true);
    if (folder) return goTo(device, folder.address);
    return { say: `Папки «${folderAsked[1]}» нет. Скажите «какие у меня папки».`, continue: WALK };
  }
  if (/^(дальше|следующ|еще|ещё|вперед)/.test(said)) {
    if (state.open && ["text", "pdf", "doc"].includes(state.kind)) return readOn(device, state, await speakable(state.open));
    return step(device, state, 1);
  }
  if (/^(предыдущ|обратно|прошл)/.test(said)) return step(device, state, -1);
  if (/^(назад|наверх|выше|вверх|выйди)/.test(said)) {
    if (state.open) {
      keep(device, { ...state, open: null, kind: null });
      const speak = await speakable(state.path);
      return { say: sayList(state.path, { items: state.items, total: state.total }, speak), continue: WALK };
    }
    if (!state.path) return folders({ request });
    const parts = state.path.split("/");
    const top = state.path === folderOf(state.path);
    if (top) return folders({ request });
    return goTo(device, parts.slice(0, -1).join("/"));
  }
  if (/^(прочитай|читай|прочти)/.test(said)) {
    const what = said.replace(/^(прочитай|читай|прочти)\s*/, "").replace(/^(файл|документ)\s*/, "");
    if (!what && state.open) return readOn(device, { ...state, part: 0, offset: 0 }, await speakable(state.open));
    const item = best(what, state.items.filter((entry) => !entry.folder));
    if (item) return openItem(device, state, item);
    return { say: "Что прочитать? Скажите название.", continue: WALK };
  }
  if (/^(найди|поищи|ищи)/.test(said)) {
    const query = said.replace(/^(найди|поищи|ищи)\s*/, "");
    const hits = await findEverywhere(query, state.path ?? undefined);
    if (hits.length === 0) return { say: `Не нашла «${query}».`, continue: WALK };
    const top = hits[0];
    return find({ query: stem(top.name), request });
  }
  if (/^(запиши|сохрани)/.test(said) && state.path) {
    return writeInto(device, state.path, said.replace(/^(запиши|сохрани)\s*/, ""));
  }
  if (/^(что тут|что здесь|список|повтори)/.test(said)) {
    const speak = state.path ? await speakable(state.path) : true;
    return { say: state.path ? sayList(state.path, { items: state.items, total: state.total }, speak) : "Скажите название папки.", continue: WALK };
  }
  const number = /^(номер|открой|покажи)?\s*\d/.test(said) || /^(номер|первую|вторую|третью|четвертую|пятую|первый|второй|третий)/.test(said) ? numberIn(said) : null;
  if (number && state.items[number - 1]) return openItem(device, state, state.items[number - 1]);
  // «Открой фото» — это папка «Фото»: слово рода снимается, только если за ним есть имя («открой фото 3»).
  const named = said.replace(/^(открой|покажи|зайди в|включи|давай)\s*/, "");
  const bare = named.replace(/^(папку|файл|фото)\s+/, "");
  const item = (await namedThing(state, bare)) ?? (bare !== named ? await namedThing(state, named) : null);
  if (item) return openItem(device, state, item);
  return { say: "Не поняла. Скажите название, «дальше», «назад» или «хватит».", continue: WALK };
}

// ——— Карточка на столе ———

function rowsOf(text) {
  const rows = [];
  for (const paragraph of String(text ?? "").split("\n")) {
    let rest = paragraph.trim();
    while (rest && rows.length < 50) {
      rows.push({ text: rest.slice(0, 200) });
      rest = rest.slice(200);
    }
  }
  return rows;
}

const KIND_WORDS = { folder: "папка", image: "фото", text: "текст", pdf: "pdf", doc: "документ", audio: "звук", video: "видео", other: "файл" };

export async function card() {
  const device = host.kv.get("last");
  const state = device ? walkOf(device) : null;
  if (!state || !state.path) {
    const list = await known();
    return {
      cap: "Скажите: «Алиса, что в папке Фото»",
      rows: list.map((folder) => ({ text: folder.name, value: "папка", tap: "cardOpen", with: { address: folder.address } })),
      empty: "Папок пока нет.",
      say: "Папки дома.",
    };
  }
  const where = crumbs(state.open ? state.open : state.path);
  if (state.open && state.kind === "image") {
    const pictures = state.items.filter((item) => item.kind === "image");
    const at = pictures.findIndex((item) => item.address === state.open);
    return {
      cap: `${where} · ${at + 1} из ${pictures.length}`,
      picture: { address: state.open, caption: stem(nameOf(state.open)) },
      buttons: [
        { text: "Назад", tap: "cardUp" },
        { text: "Предыдущая", tap: "cardPrev" },
        { text: "Следующая", tap: "cardNext" },
      ],
    };
  }
  if (state.open && ["text", "pdf", "doc"].includes(state.kind)) {
    let shown = state.reading;
    if (!shown) {
      shown = await host.files.text(state.open, { part: 0 }).then((text) => text.text).catch((error) => String(error?.message ?? error));
    }
    return {
      cap: where,
      rows: rowsOf(shown),
      buttons: [
        { text: "Назад", tap: "cardUp" },
        { text: "Дальше", tap: "cardNext" },
      ],
    };
  }
  return {
    cap: `${where} · ${state.total} ${plural(state.total, "вещь", "вещи", "вещей")}`,
    rows: state.items.slice(0, 50).map((item) => ({
      text: item.name.slice(0, 200),
      value: KIND_WORDS[item.kind] ?? "",
      tap: "cardOpen",
      with: { address: item.address },
    })),
    buttons: [{ text: "Наверх", tap: "cardUp" }],
    empty: "Пусто.",
  };
}

function panelDevice() {
  return host.kv.get("last") ?? "panel";
}

export async function cardOpen({ address }) {
  const device = panelDevice();
  const state = walkOf(device) ?? { items: [], path: null };
  const item = state.items.find((entry) => entry.address === address) ?? (await known()).find((entry) => entry.address === address);
  if (!item) return card();
  if (item.folder) await goTo(device, item.address);
  else await openItem(device, state, item);
  return card();
}

export async function cardUp() {
  await walk({ text: "назад", request: { device: panelDevice() } });
  return card();
}

export async function cardNext() {
  await walk({ text: "дальше", request: { device: panelDevice() } });
  return card();
}

export async function cardPrev() {
  await walk({ text: "предыдущая", request: { device: panelDevice() } });
  return card();
}

// ——— Инструменты «Мозга» ———

export async function toolFind({ query, folder }) {
  const hits = await findEverywhere(String(query ?? ""), folder);
  return hits.slice(0, 10).map((hit) => ({ address: hit.address, name: hit.name, kind: hit.kind }));
}

export async function toolOpen({ address, request }) {
  if (!address) return (await known()).map((folder) => ({ address: folder.address, name: folder.name }));
  const device = panelDevice();
  const stat = await host.files.stat(address);
  if (stat.folder) {
    await goTo(device, address);
    const state = walkOf(device);
    return { address, total: state.total, items: state.items.slice(0, 50).map((item) => ({ name: item.name, kind: item.kind, address: item.address })) };
  }
  const path = address.split("/").slice(0, -1).join("/");
  const page = await listing(path);
  keep(device, { path, items: page.items, total: page.total, said: SAID, open: address, kind: stat.kind, part: 0, offset: 0 });
  // Звук по слову колонке — на колонку (её плеер, «Алиса, пауза» и «стоп» — её же); в пульте — окном.
  if (stat.kind === "audio" && request?.station) return playOnStation(address);
  if (SHOWN.includes(stat.kind)) {
    const opened = await screenTry([address]);
    if (opened) return { address, kind: stat.kind, shown: "в окне на экране компьютера", window: opened[0]?.id };
  }
  return { address, kind: stat.kind, shown: "на карточке на столе" };
}

// Звук из папки — колонке: ответ инструмента с `play` шлюз включает в её ответе.
async function playOnStation(address) {
  const element = await host.files.play(address);
  const speak = await speakable(address);
  return { play: [element], say: speak ? `Включаю «${element.title}».` : "Включаю." };
}

// Инструмент folder_show: фото, видео и звук окнами на экране компьютера, каждое — своим окном.
export async function toolShow({ addresses, add }) {
  const list = (Array.isArray(addresses) ? addresses : [addresses]).map((one) => String(one ?? "")).filter(Boolean);
  if (list.length === 0) return { opened: [], note: "Нет адресов: найди их через folder_find или folder_open" };
  const opened = await onScreen(list.slice(0, 8), add === true);
  return { opened: opened.map((window) => ({ window: window.id, title: window.title, kind: window.kind })) };
}

// Инструмент folder_play: видео или звук домашней папки в окне — играть, пауза, продолжить, стоп (окно закрывается).
export async function toolPlay({ address, action = "play", request }) {
  const at = String(address ?? "");
  const station = request?.station === true;
  if (action === "play") {
    if (!at) return { error: "Нужен адрес files/home/… видео или звука" };
    const stat = await host.files.stat(at);
    // Звук по слову колонке — на колонке; видео и звук из пульта — окном на экране компьютера.
    if (stat.kind === "audio" && station) return playOnStation(at);
    const opened = await onScreen([at], false);
    return { playing: opened.map((window) => ({ window: window.id, title: window.title, kind: window.kind })) };
  }
  const handle = { pause: "pause", resume: "resume", stop: "close" }[action];
  if (!handle) return { error: "action — play, pause, resume или stop" };
  // Сперва окна на экране; ничего там не играет, а слово — колонке, — её плеер.
  const target = at || (handle === "close" ? "temporary" : undefined);
  const done = await toScreen(handle, target ? { window: target } : {});
  const touched = done && (done.paused ?? done.resumed ?? done.closed ?? []).length > 0;
  if (touched) return done;
  if (station) return { player: action };
  return { error: "Ни окна с видео или звуком, ни колонки рядом: останавливать нечего" };
}

export async function toolRead({ address, part }) {
  return host.files.text(address, { part: Number(part ?? 0) || 0 });
}

export async function toolWrite({ folder, name, text }) {
  const result = await writeInto(panelDevice(), String(folder), `${String(name ?? "Заметка")}: ${String(text ?? "")}`);
  return result.say ?? "Записано.";
}
