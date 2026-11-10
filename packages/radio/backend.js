// Радио: свой список популярных станций и открытый каталог Radio Browser (https://www.radio-browser.info).
//
// Шлюз зовёт эти функции:
// - playStation — на фразу «включи радио …» (contributes.intents): находит станцию и отдаёт очередь плееру шлюза;
//   ничего не нашла — возвращает null, и тогда фразу обслуживает встроенное радио шлюза;
// - radioSearch — инструмент «Мозга» radio_search: список станций по названию, жанру или настроению;
// - radioPlay — инструмент radio_play: находит и сразу включает (ответ того же вида, что на фразу, — `{say, play}`);
// - stations — источник радио для встроенного навыка (contributes.media);
// - onPlayer — что колонка сказала о станции (player.started, player.stopped, player.silent, player.failed): что
//   играет — виджету; молчит — станция запоминается как нерабочая и уходит в конец выдачи, а вместо неё играет
//   следующая;
// - onStation — колонка подключилась или отключилась (station.connected, station.disconnected): не вернулась вовремя
//   (минута после обрыва, 3 — после смены режима) — у неё ничего не играет;
// - panelStations — виджету «Радио» на столе пульта (panel.js).
//
// Звук идёт через плеер шлюза (audio_play), поэтому «пауза», «дальше», «громче» — встроенные навыки.

// Свои станции — все по https: поток из интернета по http колонка не играет и молчит (на колонке 07.11.2026: «Хит FM»
// по https играет, «Радио Джаз» по http — нет). Проверенно играет MP3 и HLS; «Рекорд» пока в AAC — проба, играет ли
// колонка AAC. Теги — для поиска по жанру и настроению.
const OWN = [
  { id: "studio21", title: "Студия 21", aliases: ["studio 21", "студио 21"], tags: ["хип хоп", "рэп"], url: "https://hls.studio21.ru/studio21/playlist.m3u8" },
  { id: "europa", title: "Европа Плюс", aliases: ["europa plus", "европа"], tags: ["поп", "хиты", "танцевальная"], url: "https://ep256.hostingradio.ru:8052/europaplus256.mp3" },
  { id: "russkoe", title: "Русское радио", aliases: ["русское"], tags: ["поп", "русская музыка"], url: "https://rusradio.hostingradio.ru/rusradio128.mp3" },
  { id: "record", title: "Радио Рекорд", aliases: ["рекорд", "record"], tags: ["танцевальная", "электроника", "дэнс"], url: "https://radiorecord.hostingradio.ru/rr_main96.aacp" },
  { id: "nashe", title: "Наше радио", aliases: ["наше"], tags: ["рок", "русский рок"], url: "https://nashe.streamr.ru/nashe-128.mp3" },
  { id: "jazz", title: "Радио Джаз", aliases: ["джаз фм", "radio jazz"], tags: ["джаз", "блюз"], url: "https://main.hostingradio.ru/jazz-128.mp3" },
  { id: "relax", title: "Relax FM", aliases: ["релакс", "релакс фм"], tags: ["спокойное", "спокойная", "расслабиться", "чилаут", "лаунж"], url: "https://pub0201.101.ru/stream/trust/mp3/128/24" },
  { id: "mayak", title: "Маяк", aliases: ["радио маяк"], tags: ["разговорное", "новости"], url: "https://icecast-vgtrk.cdnvideo.ru/mayakfm_mp3_192kbps" },
  { id: "chanson", title: "Радио Шансон", aliases: ["шансон"], tags: ["шансон"], url: "https://chanson.hostingradio.ru:8041/chanson256.mp3" },
  { id: "retro", title: "Ретро FM", aliases: ["ретро фм", "ретро"], tags: ["ретро", "старые песни", "диско"], url: "https://retroserver.streamr.ru:8043/retro256.mp3" },
  { id: "dfm", title: "DFM", aliases: ["дфм", "ди фм"], tags: ["танцевальная", "дэнс", "электроника"], url: "https://dfm.hostingradio.ru/dfm128.mp3" },
  { id: "hitfm", title: "Хит FM", aliases: ["хит фм", "hit fm"], tags: ["хиты", "поп"], url: "https://hitfm.hostingradio.ru/hitfm128.mp3" },
  { id: "montecarlo", title: "Монте-Карло", aliases: ["монте карло", "monte carlo"], tags: ["лаунж", "спокойное"], url: "https://montecarlo.hostingradio.ru/montecarlo128.mp3" },
  { id: "orpheus", title: "Радио Орфей", aliases: ["орфей"], tags: ["классика", "классическая музыка"], url: "https://orfeyfm.hostingradio.ru:8034/orfeyfm192.mp3" },
  { id: "kultura", title: "Радио Культура", aliases: ["культура", "радио россии культура"], tags: ["классика", "разговорное"], url: "https://icecast-vgtrk.cdnvideo.ru/kulturafm_mp3_192kbps" },
  { id: "vesti", title: "Вести FM", aliases: ["вести фм", "вести"], tags: ["новости", "разговорное"], url: "https://icecast-vgtrk.cdnvideo.ru/vestifm" },
  { id: "maximum", title: "Радио Максимум", aliases: ["максимум"], tags: ["рок", "поп"], url: "https://maximum.hostingradio.ru/maximum128.mp3" },
  { id: "dorozhnoe", title: "Дорожное радио", aliases: ["дорожное"], tags: ["ретро", "русская музыка"], url: "https://dorognoe.hostingradio.ru:8000/radio" },
];

// Жанры по-русски — теги каталога, он помечает станции по-английски.
const GENRES = {
  "джаз": "jazz", "рок": "rock", "поп": "pop", "классика": "classical", "классическая": "classical",
  "электроника": "electronic", "танцевальная": "dance", "дэнс": "dance", "шансон": "chanson", "новости": "news",
  "хип хоп": "hip hop", "рэп": "rap", "блюз": "blues", "металл": "metal", "кантри": "country", "регги": "reggae",
  "ретро": "oldies", "спокойное": "chillout", "спокойная": "chillout", "релакс": "chillout", "лаунж": "lounge",
  "детское": "kids", "детская": "kids", "фолк": "folk", "соул": "soul", "диско": "disco", "техно": "techno",
  "лоу фай": "lofi", "лофай": "lofi", "лоуфай": "lofi", "lo fi": "lofi", "lofi": "lofi", "чилаут": "chillout",
};

const SERVERS = ["de1.api.radio-browser.info", "de2.api.radio-browser.info", "fi1.api.radio-browser.info"];
// Голосовой вызов ограничен 2 с: каталог должен ответить раньше.
const BUDGET_MS = 1600;
// Из каталога — только то, что колонка проверенно играет: MP3 и HLS по https. AAC — когда пройдёт проба на «Рекорде».
const CODECS = ["MP3"];

function normalize(text) {
  return String(text ?? "").toLowerCase().replace(/ё/g, "е").replace(/[^\p{L}\p{N}]+/gu, " ").trim();
}

// Слово находится по основе: «джазовое» находит «джаз», «спокойную» — «спокойная».
function stem(word) {
  return word.length > 4 ? word.slice(0, -2) : word.length === 4 ? word.slice(0, 3) : word;
}

const SKIP = new Set(["радио", "радиостанцию", "станцию", "музыка", "музыку", "музыки", "песни", "что", "нибудь", "какое", "какую", "мне", "по", "на", "в", "и"]);

function score(query, station) {
  const words = normalize(query).split(" ").filter((word) => word.length > 1 && !SKIP.has(word));
  if (words.length === 0) return 0;
  const haystack = normalize([station.title, ...station.aliases, ...station.tags].join(" "));
  return words.filter((word) => haystack.includes(stem(word))).length / words.length;
}

function item(station) {
  return {
    id: station.id,
    title: station.title,
    url: station.url,
    format: station.url.includes(".m3u8") ? "HLS" : "MP3",
    subtitle: station.tags.join(", "),
  };
}

function ownFind(query, limit) {
  if (!normalize(query)) return OWN.slice(0, limit).map(item);
  return OWN.map((station) => [score(query, station), station])
    .filter(([found]) => found >= 0.5)
    .sort((a, b) => b[0] - a[0])
    .slice(0, limit)
    .map(([, station]) => item(station));
}

// Неудачный запрос к каталогу не повторяется полминуты: встроенное радио, которому плагин отдал фразу, спросит
// этот же источник ещё раз.
const misses = new Map();

async function browse(path, until) {
  let failure;
  for (const server of SERVERS) {
    const left = until - Date.now();
    if (left < 150) break;
    try {
      const answer = await host.http.fetch(`https://${server}${path}`, {
        timeoutMs: left - 50,
        headers: { "User-Agent": "duowave-radio/1.0" },
      });
      if (answer.ok) return await answer.json();
      failure = new Error(`${server}: ${answer.status}`);
    } catch (error) {
      failure = error;
    }
  }
  throw failure ?? new Error("каталог не ответил вовремя");
}

function fromCatalog(station) {
  const tags = String(station.tags ?? "").split(",").map((tag) => tag.trim()).filter(Boolean).slice(0, 3);
  return {
    id: `rb-${station.stationuuid}`,
    title: String(station.name).trim(),
    url: station.url_resolved,
    format: station.hls || String(station.url_resolved).includes(".m3u8") ? "HLS" : "MP3",
    subtitle: tags.length ? tags.join(", ") : station.country,
    imageUrl: String(station.favicon ?? "").startsWith("https://") ? station.favicon : undefined,
  };
}

async function catalogFind(query, limit, until) {
  const key = normalize(query);
  if (!key || host.settings.get("catalog") === false) return [];
  const missed = misses.get(key);
  if (missed && Date.now() - missed < 30_000) return [];
  const country = String(host.settings.get("country") ?? "RU").toUpperCase();
  const common = `hidebroken=true&is_https=true&order=clickcount&reverse=true&limit=${limit}`;
  const tag = GENRES[key];
  const path = tag
    ? `/json/stations/search?tag=${encodeURIComponent(tag)}&countrycode=${country}&${common}`
    : `/json/stations/search?name=${encodeURIComponent(query)}&${common}`;
  const found = (await browse(path, until))
    .filter((station) => station.lastcheckok === 1 && /^https:\/\//.test(station.url_resolved ?? ""))
    .filter((station) => CODECS.includes(String(station.codec).toUpperCase()) || station.hls)
    // The country's stations first, then the rest of the world.
    .sort((a, b) => (b.countrycode === country) - (a.countrycode === country))
    .map(fromCatalog);
  if (found.length === 0) misses.set(key, Date.now());
  return found;
}

// Станции, на которых колонка молчала или не смогла играть (этап 5а): помнятся неделю и уходят в конец выдачи — если
// назвать такую станцию, она всё равно попробует включиться.
const BROKEN_MS = 7 * 24 * 3600 * 1000;

function brokenAt(id) {
  const at = host.kv.get(`broken:${id}`);
  return typeof at === "number" && Date.now() - at < BROKEN_MS ? at : null;
}

function workingFirst(items) {
  return [...items.filter((one) => !brokenAt(one.id)), ...items.filter((one) => brokenAt(one.id))];
}

// Что играет — у каждой колонки своё: ставит player.started, снимает player.stopped того же элемента, а его шлюз шлёт и
// тогда, когда поверх радио включили другое (свою музыку, сказку). «Играет» старше 12 часов всё равно не показывается —
// на случай, когда о конце так и не узнали.
const NOW_MS = 12 * 60 * 60 * 1000;
const nowKey = (device) => `now:${device}`.slice(0, 100);
// Колонка отключилась: вернулась вовремя — поток играет дальше, «играет» остаётся; не вернулась — у неё ничего не
// играет. Соединение оборвалось (1006: сброс, тишина) — ждём минуту. Закрыто рукопожатием (смена режима) — 3 минуты:
// следующее колонка открывает сама, от секунды до 2,5 минуты, и присутствие шлюза ждёт её столько же.
const AWAY_MS = 60 * 1000;
const SWITCH_MS = 3 * 60 * 1000;
// Последнее соединение колонки: прежнее закрылось, когда новое уже открыто, — колонка на связи.
const linkKey = (device) => `link:${device}`.slice(0, 100);

// Когда «играет» гаснет, если колонка не вернётся.
const goneAt = (now) => now.away + (now.wait ?? AWAY_MS);

// «Играет» на виду: не старше 12 часов, и колонка не пропала дольше, чем её ждут.
function playing(now, at) {
  return Boolean(now) && at - now.since < NOW_MS && !(now.away && at > goneAt(now));
}

// Свой список, потом каталог; каталог не ответил — что нашлось у себя.
async function find(query, limit) {
  const own = ownFind(query, limit);
  if (own.length > 0 || !normalize(query)) return own;
  try {
    return await catalogFind(query, limit, Date.now() + BUDGET_MS);
  } catch (error) {
    console.warn(`каталог: ${error.message}`);
    return [];
  }
}

// «Включи радио джаз». Без названия — последняя станция, которую включали.
export async function playStation({ station }) {
  const query = station ?? host.kv.get("last") ?? "";
  const found = workingFirst(await find(query, 20));
  if (found.length === 0) return null;
  host.kv.set("last", found[0].title);
  // «Дальше» переключает на следующую: остальное найденное, потом свой список; нерабочие — в конце.
  const seen = new Set(found.map((one) => one.url));
  const rest = workingFirst(OWN.map(item).filter((one) => !seen.has(one.url)));
  return { play: [...found, ...rest] };
}

// Инструмент radio_play: «Мозг» нашёл станцию — и сразу включает её. Ответ того же вида, что на фразу: шлюз включает
// `play` в ответе колонки, а «Мозг» читает, что включено.
export async function radioPlay({ station }) {
  const answer = await playStation({ station: String(station ?? "") });
  if (!answer) return { found: false, note: "Такой станции не нашлось — предложи другое название или жанр" };
  return { ...answer, say: `Включаю ${answer.play[0].title}` };
}

// Что колонка сказала о станции. Молчит и шлюз пробует свой путь — ждём; молчит и через шлюз или не смогла —
// станция нерабочая, а вместо неё играет похожая (общий тег), потом остальные.
export async function onPlayer({ name, payload }) {
  const id = payload?.item?.id;
  if (!id) return null;
  const device = String(payload.device ?? "");
  if (name === "player.started") {
    if (brokenAt(id)) host.kv.delete(`broken:${id}`);
    // Виджету на столе: что играет на этой колонке и с какого времени («играет 44 минуты»).
    host.kv.set(nowKey(device), { id, title: payload.item.title ?? id, device, since: Date.now() });
    return null;
  }
  if (name === "player.stopped") {
    if (host.kv.get(nowKey(device))?.id === id) host.kv.delete(nowKey(device));
    return null;
  }
  if (name === "player.silent" && payload.retrying) {
    console.log(`колонка молчит на «${payload.item.title}» — шлюз пробует свой путь`);
    return null;
  }
  if (name !== "player.silent" && name !== "player.failed") return null;
  host.kv.set(`broken:${id}`, Date.now());
  const silent = OWN.find((station) => station.id === id);
  const near = (station) => (silent ? station.tags.filter((tag) => silent.tags.includes(tag)).length : 0);
  const next = OWN.filter((station) => station.id !== id && !brokenAt(station.id))
    .sort((a, b) => near(b) - near(a))
    .map(item);
  console.warn(`«${payload.item.title}» не играет на колонке${payload.viaGateway ? " и через шлюз" : ""} — дальше ${next[0]?.title ?? "нечего"}`);
  return next.length > 0 ? { play: next } : null;
}

// Колонка подключилась или отключилась. Отключилась — «играет» не снимается сразу, а помечается: смена режима или
// короткий обрыв поток не останавливают. Вернулась вовремя — пометка уходит; позже — у неё ничего не играет.
export function onStation({ name, payload }) {
  const device = String(payload?.deviceTag ?? "");
  if (!device) return null;
  const session = String(payload?.sessionId ?? "");
  const now = host.kv.get(nowKey(device));
  if (name === "station.connected") {
    if (session) host.kv.set(linkKey(device), session);
    if (!now?.away) return null;
    if (Date.now() > goneAt(now)) host.kv.delete(nowKey(device));
    else host.kv.set(nowKey(device), { ...now, away: null, wait: null });
    return null;
  }
  if (name !== "station.disconnected" || !now || now.away) return null;
  // Закрылось прежнее соединение, а новое уже открыто: колонка на связи.
  const live = host.kv.get(linkKey(device));
  if (live && session && live !== session) return null;
  const handshake = typeof payload.code === "number" && payload.code !== 1006;
  host.kv.set(nowKey(device), { ...now, away: Date.now(), wait: handshake ? SWITCH_MS : AWAY_MS });
  return null;
}

// Виджету «Радио» на столе пульта: что играет сейчас на колонке пульта и с какого времени, свои станции, какие молчали,
// что включали последним. Без колонки — последнее начатое на любой. Колонка пропала, но её ещё ждут, — until: после
// него виджет уберёт «играет» сам, не дожидаясь, пока его перечитают.
export function panelStations({ device } = {}) {
  // Прежняя версия держала одно «что играет» на все колонки.
  if (host.kv.get("now") !== null) host.kv.delete("now");
  let now = null;
  for (const key of device ? [nowKey(String(device))] : host.kv.keys("now:")) {
    const one = host.kv.get(key);
    if (playing(one, Date.now()) && (!now || one.since > now.since)) now = one;
  }
  return {
    now: now?.away ? { ...now, until: goneAt(now) } : now,
    last: host.kv.get("last") ?? null,
    stations: OWN.map((station) => ({ id: station.id, title: station.title, tags: station.tags, broken: brokenAt(station.id) !== null })),
  };
}

// Инструмент radio_search.
export async function radioSearch({ query, limit = 8 }) {
  const count = Math.min(Math.max(Number(limit) || 8, 1), 20);
  const found = await find(String(query ?? ""), count);
  if (found.length === 0) return { query, stations: [], note: "Ничего не нашлось" };
  return {
    query,
    stations: workingFirst(found).map((one) => ({ title: one.title, about: one.subtitle ?? "", works: !brokenAt(one.id) })),
    next: "Чтобы включить, вызови radio_play с названием станции",
  };
}

// Источник радио для встроенного навыка: по названию, без названия — свой список.
export async function stations({ query, limit }) {
  return await find(String(query ?? ""), Math.min(Number(limit) || 20, 50));
}
