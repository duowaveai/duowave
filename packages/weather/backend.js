// Погода: Open-Meteo (без ключа) — геокодер для города и прогноз. Перенос встроенного навыка шлюза
// (gateway/src/skills/lookups/weather_skill.rs) в плагин, этап 10 плана: те же ответы, то же место по умолчанию.
// Встроенный навык остаётся запасным: плагин вернул null или упал — отвечает он.

const GEOCODE = "https://geocoding-api.open-meteo.com/v1/search";
const FORECAST = "https://api.open-meteo.com/v1/forecast";
// Вызов голоса, «Мозга» и пульта ограничен 2 с: город — из кэша, прогноз — одним запросом, а все запросы вызова
// вместе — не дольше 1,7 с.
const TIMEOUT = 1700;

// Сколько ещё можно ждать сети в этом вызове.
function left(until) {
  const ms = until - Date.now();
  if (ms < 150) throw new Error("Open-Meteo не ответил вовремя");
  return ms;
}

const CODES = [
  [[0], "ясно"], [[1], "преимущественно ясно"], [[2], "переменная облачность"], [[3], "пасмурно"],
  [[45, 48], "туман"], [[51, 53, 55], "морось"], [[56, 57], "ледяная морось"], [[61], "небольшой дождь"],
  [[63], "дождь"], [[65], "сильный дождь"], [[66, 67], "ледяной дождь"], [[71], "небольшой снег"], [[73], "снег"],
  [[75], "сильный снег"], [[77], "снежная крупа"], [[80, 81], "ливень"], [[82], "сильный ливень"],
  [[85, 86], "снегопад"], [[95], "гроза"], [[96, 99], "гроза с градом"],
];

function describeCode(code) {
  const found = CODES.find(([codes]) => codes.includes(code));
  return found ? found[1] : "погода без описания";
}

function signed(value) {
  const rounded = Math.round(Number(value));
  return `${rounded > 0 ? "+" : ""}${rounded}`;
}

function normalize(text) {
  return String(text ?? "").toLowerCase().replace(/ё/g, "е").replace(/[^\p{L}\p{N}\s,.-]+/gu, " ").replace(/\s+/g, " ").trim();
}

// ---- The place in a question ------------------------------------------------------------------------------------

// Words that start a place: «в Москве», «для Москвы», «под Москвой».
const PREPOSITIONS = new Set(["в", "во", "для", "около", "возле", "под"]);
// Words that end a place: they tell when, not where («в Москве завтра», «в эту пятницу», «в ноябре»). «на» before
// one of them is about time too («на неделю»); before anything else it is part of a name («Ростов-на-Дону»).
const NOT_PLACE = new Set([
  "сегодня", "завтра", "послезавтра", "сейчас", "утром", "днем", "вечером", "ночью", "обед", "полдень", "неделю",
  "неделе", "недели", "выходные", "выходных", "понедельник", "вторник", "среду", "четверг", "пятницу", "субботу",
  "воскресенье", "эту", "этой", "этот", "следующую", "следующей", "следующий", "ближайшие", "ближайшую",
  "ближайшее", "течение", "целом", "общем", "январе", "феврале", "марте", "апреле", "мае", "июне", "июле",
  "августе", "сентябре", "октябре", "ноябре", "декабре", "будет", "ли", "и", "или", "а", "погода", "погоду",
]);
const PLACE_WORDS = 4;

// The words of a place from `start` on, up to the first word that is not one.
function phraseAt(words, start) {
  const taken = [];
  let at = start;
  for (; at < words.length && taken.length < PLACE_WORDS; at += 1) {
    const word = words[at];
    if (NOT_PLACE.has(word) || word === "в" || word === "во" || /\d/.test(word)) break;
    if (word === "на" && (at + 1 >= words.length || NOT_PLACE.has(words[at + 1]))) break;
    taken.push(word);
  }
  return { words: taken, end: at };
}

// «… в москве», «завтра в нижнем новгороде», «в париже во франции»: the place as heard (still in its case), the
// country if one is named after it, and the words to repeat when the place is not found.
function placeIn(text) {
  const words = normalize(text).replace(/[,.]/g, " ").split(" ").filter(Boolean);
  for (let index = 0; index < words.length; index += 1) {
    if (!PREPOSITIONS.has(words[index])) continue;
    const place = phraseAt(words, index + 1);
    if (place.words.length === 0) continue;
    let country = null;
    if (words[place.end] === "в" || words[place.end] === "во") {
      const named = phraseAt(words, place.end + 1);
      if (named.words.length > 0) country = named.words.join(" ");
    }
    return { place: place.words.join(" "), country, shown: `${words[index]} ${place.words.join(" ")}` };
  }
  return null;
}

// A place as settings or a tool give it: «Москва», «Париж, Франция», «в Нижнем Новгороде», «55.75, 37.62».
function givenPlace(text) {
  const raw = String(text ?? "").trim();
  const coordinates = raw.match(/^\s*(-?\d+(?:\.\d+)?)\s*,\s*(-?\d+(?:\.\d+)?)\s*$/);
  if (coordinates) return { coordinates: { name: "Здесь", latitude: Number(coordinates[1]), longitude: Number(coordinates[2]) } };
  const clean = normalize(raw);
  if (PREPOSITIONS.has(clean.split(" ")[0])) {
    const heard = placeIn(clean);
    if (heard) return heard;
  }
  const [place, country] = clean.split(",").map((part) => part.replace(/\./g, " ").trim());
  return { place, country: country || null, shown: raw };
}

// ---- A name's dictionary forms ----------------------------------------------------------------------------------

// Case endings of a noun and the dictionary endings each may come from, most likely first; the longest ending that
// fits is the one taken. «Москве» → «Москва», «Новосибирске» → «Новосибирск», «Казани» → «Казань».
const NOUN_ENDINGS = [
  ["ями", ["и", "я"]], ["ами", ["ы", "и", "а"]], ["ах", ["ы", "и", "а"]], ["ях", ["и", "я"]],
  ["ии", ["ия"]], ["ье", ["ья", "ия", "ь"]], ["ьи", ["ья", "ия", "ь"]], ["ью", ["ь", "ья", "ия"]],
  ["ой", ["а"]], ["ей", ["я", "ь"]], ["ом", ["", "о"]], ["ем", ["ь", "й", "е"]],
  ["е", ["", "а", "ь", "о", "я", "й"]], ["и", ["ь", "а", "я"]], ["ы", ["а"]],
  ["у", ["", "а", "о"]], ["ю", ["ь", "я"]], ["а", ["", "о"]], ["я", ["ь", "й"]],
];
// The same for an adjective in a name: «Нижнем» → «Нижний», «Великих» → «Великие», «Старой» → «Старая».
const ADJECTIVE_ENDINGS = [
  ["ыми", ["ые"]], ["ими", ["ие"]], ["ого", ["ый", "ой", "ое"]], ["его", ["ий", "ее"]],
  ["ому", ["ый", "ой", "ое"]], ["ему", ["ий", "ее"]], ["ых", ["ые"]], ["их", ["ие"]], ["ым", ["ый", "ые"]],
  ["им", ["ий", "ие"]], ["ом", ["ый", "ой", "ое"]], ["ем", ["ий", "ее"]], ["ой", ["ая", "ой", "ый"]],
  ["ей", ["яя", "ий"]], ["ую", ["ая"]], ["юю", ["яя"]],
];

function endingOf(word, table, shortest) {
  const found = table.find(([ending]) => word.endsWith(ending) && word.length - ending.length >= shortest);
  return found ? { stem: word.slice(0, -found[0].length), ending: found[0], into: found[1] } : null;
}

// A word's possible dictionary forms (most likely first, the word itself last: «Сочи», «Баку» and «Кемерово» do not
// change) and its stem, the word without its case ending. A word before the last one needs a longer stem to be taken
// for an inflected one («Нью» in «Нью-Йорке» is not), and an adjective already in its dictionary form stays
// («Великие Луки» as the settings give it).
function wordForms(word, shortest) {
  if (shortest > 2 && /(ый|ий|ая|яя|ое|ее|ые|ие)$/.test(word)) return { stem: word, forms: [word] };
  const noun = endingOf(word, NOUN_ENDINGS, shortest);
  const adjective = endingOf(word, ADJECTIVE_ENDINGS, shortest);
  const forms = [];
  const add = (form) => form && !forms.includes(form) && forms.push(form);
  if (adjective) {
    const velar = /[гкхжшчщ]$/.test(adjective.stem);
    // «Сергиевом Посаде» → «Сергиев Посад»: a possessive adjective has no ending of its own.
    if (/(ов|ев|ин|ын)$/.test(adjective.stem) && ["ого", "ому", "ым", "ом"].includes(adjective.ending)) add(adjective.stem);
    for (const into of adjective.into) add(adjective.stem + (velar ? into.replace(/^ы/, "и") : into));
  }
  if (noun) for (const into of noun.into) add(noun.stem + into);
  add(word);
  const stems = [noun?.stem, adjective?.stem].filter(Boolean);
  const stem = stems.length > 0 ? stems.reduce((a, b) => (b.length < a.length ? b : a)) : word;
  return { stem, forms };
}

// Every reading of a name's words with its rank (the sum of its forms' ranks), the likeliest first, at most `limit`.
function combine(lists, limit) {
  let combos = [{ parts: [], rank: 0 }];
  for (const list of lists) {
    const next = [];
    for (const combo of combos) list.forEach((form, rank) => next.push({ parts: [...combo.parts, form], rank: combo.rank + rank }));
    next.sort((a, b) => a.rank - b.rank);
    combos = next.slice(0, limit);
  }
  return combos;
}

// A name as compared with the geocoder's: lower case, ё as е, a hyphen as a space.
function plain(text) {
  return String(text ?? "").toLowerCase().replace(/ё/g, "е").replace(/[\s-]+/g, " ").trim();
}

// What people call a city and what the geocoder calls it: «в Питере» is no village named Питер, «Петербург» alone
// finds only American towns.
const ALIASES = {
  "питер": "санкт-петербург", "петербург": "санкт-петербург", "спб": "санкт-петербург", "ленинград": "санкт-петербург",
  "мск": "москва", "екб": "екатеринбург", "нск": "новосибирск", "нижний": "нижний новгород",
  "новгород": "великий новгород",
};
const COUNTRY_ALIASES = {
  "америка": "сша", "штаты": "сша", "англия": "британия", "великобритания": "британия", "белоруссия": "беларусь",
};
// At most this many names in the set a found place must match, and this many geocoder queries for one place.
const MAX_NAMES = 64;
const MAX_QUERIES = 3;

// What to ask the geocoder for a place (in any case) — `first`, and `then` only if that finds nothing — and the
// names an answer may carry, each with its rank (0 the likeliest). The geocoder matches a query as a prefix of a
// name, so the last word goes as its stem: «нижний новгород» for «нижнем новгороде», «москв» for «москве» — one
// query finds «Москва» whatever the ending.
function candidates(place) {
  const text = normalize(place).replace(/[,.]/g, " ").trim();
  if (!/[а-я]/.test(text)) return { first: text ? [text] : [], then: [], names: new Map(), latin: true };
  const parts = text.split(/([\s-]+)/);
  const words = parts.filter((_, index) => index % 2 === 0);
  const joints = parts.filter((_, index) => index % 2 === 1).map((joint) => (joint.includes("-") ? "-" : " "));
  const join = (list) => list.map((word, index) => (index === 0 ? word : joints[index - 1] + word)).join("");
  // «Ростов-на-Дону», «Комсомольске-на-Амуре»: what follows «на» does not change.
  const at = words.indexOf("на", 1);
  const tail = at > 0 ? words.slice(at) : [];
  const changing = at > 0 ? words.slice(0, at) : words;
  const forms = changing.map((word, index) => wordForms(word, index === changing.length - 1 ? 2 : 3));
  const readings = combine(forms.map((word) => word.forms), MAX_NAMES);
  const names = new Map();
  for (const { parts: reading, rank } of readings) {
    const name = plain(join([...reading, ...tail]));
    if (!names.has(name)) names.set(name, rank);
  }
  for (const name of names.keys()) {
    if (ALIASES[name]) return { first: [ALIASES[name]], then: [], names: new Map([[plain(ALIASES[name]), 0]]), latin: false };
  }
  const last = forms[forms.length - 1];
  // A stem of one or two letters finds only names of exactly those letters («уф» for «Уфе»): the forms go instead,
  // the longer ones first.
  let queries = forms.length === 1 && last.stem.length < 3
    ? [...last.forms.filter((form) => form.length >= 3), ...last.forms.filter((form) => form.length === 2)]
    : combine(forms.slice(0, -1).map((word) => word.forms), MAX_QUERIES).map(({ parts: head }) => join([...head, last.stem]));
  // The likeliest whole name with its unchanging end first; the stem alone finds it among its prefixes too.
  if (tail.length > 0) queries.unshift(join([...readings[0].parts, ...tail]));
  queries = [...new Set(queries)].slice(0, MAX_QUERIES);
  // The likeliest query goes alone; the others only when it finds nothing.
  const first = queries.slice(0, 1);
  // «в Орле» → «Орёл», «в Ельце» → «Елец»: the vowel the case drops is not in the stem, so no prefix finds it — and
  // the stem's prefixes may hold a hamlet «Орла». Both go at once.
  if (forms.length === 1 && tail.length === 0 && last.stem.length <= 4 && /[^аеиоуыэюяь]л$|ьц$/.test(last.stem)) {
    const full = `${last.stem.slice(0, -1).replace(/ь$/, "")}е${last.stem.slice(-1)}`;
    first.push(full);
    names.set(full, 0);
  }
  return { first, then: queries.slice(1), names, latin: false };
}

// ---- Choosing among the geocoder's answers ----------------------------------------------------------------------

// How big a place is: its population, or a guess from its kind where GeoNames has none.
const KIND_SIZE = { PPLC: 100000, PPLA: 50000, PPLA2: 10000, PPLA3: 2000, PPLA4: 1000 };
function size(place) {
  if (Number(place.population) > 0) return Number(place.population);
  const kind = String(place.feature_code ?? "");
  return KIND_SIZE[kind] ?? (kind.startsWith("PPL") ? 100 : 1);
}

// The place meant, or null when none of the answers is one. Only a name that is one of the forms (exactly, or as its
// first words: «Ростов-на-Дону» for «Ростове») counts: «Москверо», which the geocoder offers for «Москве», does not.
// Of those an exact name weighs three times its size (a little less for a less likely form: «в Пушкине» is Пушкин,
// not Пушкино) and a place in Russia ten times — unless a country is named: then only places in it count. So
// «в Париже» is Paris, not the village in the Chelyabinsk region, and «в Ростове» is Rostov-on-Don, not a Ukrainian
// hamlet called Ростов. A Latin name (the time zone's «Moscow») matches the geocoder's Russian names in no form: any
// populated place it offers counts.
function choose(results, names, latin, countries) {
  let best = null;
  for (const place of results) {
    const name = plain(place.name);
    const rank = names.get(name);
    const exact = rank !== undefined;
    let partial = false;
    if (!exact && !latin) for (const wanted of names.keys()) if (name.startsWith(`${wanted} `)) { partial = true; break; }
    if (latin ? !String(place.feature_code ?? "").startsWith("PPL") : !exact && !partial) continue;
    if (countries && !countries.has(plain(place.country)) && !countries.has(plain(place.country_code))) continue;
    const weight = size(place) * (exact ? 3 / (1 + rank / 10) : 1) * (!countries && place.country_code === "RU" ? 10 : 1);
    if (!best || weight > best.weight) best = { place, weight };
  }
  return best ? { name: best.place.name, latitude: best.place.latitude, longitude: best.place.longitude } : null;
}

// A named country's forms: «во Франции» → франция, «в Америке» → сша.
function countryForms(country) {
  if (!country) return null;
  const { names } = candidates(country);
  const forms = new Set();
  for (const name of names.keys()) {
    forms.add(name);
    if (COUNTRY_ALIASES[name]) forms.add(COUNTRY_ALIASES[name]);
  }
  forms.add(plain(country));
  return forms;
}

// ---- The geocoder and its cache ---------------------------------------------------------------------------------

// The places found, in kv under one key: the newest CACHE_SIZE of them. A place not found is asked again after a day.
// The version changes with the rules of choosing: a place chosen by older rules is looked up again. 1.1.1 kept each
// place under its own `place:` key, chosen by the geocoder's first answer («Москве» → «Москверо»): those go.
const CACHE_KEY = "geo";
const CACHE_VERSION = 2;
const CACHE_SIZE = 50;
const MISS_MS = 24 * 60 * 60 * 1000;
// The geocoder's share of the call's time: the forecast needs the rest.
const GEOCODE_MS = 1000;
const GEOCODE_COUNT = 10;

let oldCacheGone = false;

function knownPlaces() {
  if (!oldCacheGone) {
    oldCacheGone = true;
    try {
      for (const key of host.kv.keys("place:")) host.kv.delete(key);
    } catch {
      // Tried again on the next lookup.
      oldCacheGone = false;
    }
  }
  const cache = host.kv.get(CACHE_KEY);
  return cache?.v === CACHE_VERSION && cache.places && typeof cache.places === "object" ? cache.places : {};
}

function remember(key, entry) {
  const places = { ...knownPlaces(), [key]: { ...entry, at: Date.now() } };
  const kept = Object.keys(places).sort((a, b) => places[b].at - places[a].at).slice(0, CACHE_SIZE);
  try {
    host.kv.set(CACHE_KEY, { v: CACHE_VERSION, places: Object.fromEntries(kept.map((name) => [name, places[name]])) });
  } catch {
    // A full store costs only the next lookup.
  }
}

async function geocode(name, until) {
  const url = `${GEOCODE}?name=${encodeURIComponent(name)}&count=${GEOCODE_COUNT}&language=ru&format=json`;
  const answer = await host.http.fetch(url, { timeoutMs: left(until) });
  if (!answer.ok) throw new Error(`геокодер Open-Meteo: ответ ${answer.status}`);
  return (await answer.json())?.results ?? [];
}

// The place's name and coordinates, or null when it is not found. The geocoder unreachable and nothing found —
// an exception: on a phrase the gateway's built-in skill answers then.
async function locate(target, until) {
  if (target.coordinates) return target.coordinates;
  const key = `${plain(target.place)}${target.country ? `|${plain(target.country)}` : ""}`.slice(0, 120);
  const known = knownPlaces()[key];
  if (known?.none && Date.now() - known.at < MISS_MS) return null;
  if (known && !known.none) return { name: known.name, latitude: known.latitude, longitude: known.longitude };
  const { first, then, names, latin } = candidates(target.place);
  if (first.length === 0) return null;
  const by = Math.min(until, Date.now() + GEOCODE_MS);
  const countries = countryForms(target.country);
  let failed = null;
  const ask = async (queries) => {
    const answers = await Promise.all(queries.map((query) => geocode(query, by).catch((error) => ((failed = error), []))));
    return choose(answers.flat(), names, latin, countries);
  };
  let found = await ask(first);
  if (!found && then.length > 0) found = await ask(then);
  if (found) remember(key, found);
  else if (failed) throw failed;
  else remember(key, { none: true });
  return found;
}

// ---- Answers ----------------------------------------------------------------------------------------------------

function describe(forecast, name, days) {
  const daily = forecast.daily;
  const day = (index) => {
    if (!daily || index >= (daily.time ?? []).length) return "";
    const rain = daily.precipitation_probability_max?.[index];
    return `от ${signed(daily.temperature_2m_min[index])} до ${signed(daily.temperature_2m_max[index])}, ${describeCode(daily.weather_code[index])}${rain ? `, вероятность осадков ${rain}%` : ""}`;
  };
  if (days === 1) return `${name}, завтра: ${day(1)}.`;
  if (days === 7 && daily) {
    return `${name}, неделя: ${daily.time.slice(0, 7).map((date, index) => `${date}: ${day(index)}`).join("; ")}.`;
  }
  const now = forecast.current;
  const parts = [];
  if (now) parts.push(`сейчас ${signed(now.temperature_2m)}, ${describeCode(now.weather_code)}${now.wind_speed_10m !== undefined ? `, ветер ${Math.round(now.wind_speed_10m)} м/с` : ""}`);
  if (daily) parts.push(`сегодня ${day(0)}`);
  return `${name}: ${parts.join("; ")}.`;
}

// Прогноз Open-Meteo; ответ с ошибкой — исключение: на фразу тогда отвечает встроенный навык.
async function fetchForecast(url, until) {
  const answer = await host.http.fetch(url, { timeoutMs: left(until) });
  if (!answer.ok) throw new Error(`Open-Meteo: ответ ${answer.status}`);
  return answer.json();
}

function notFound(target) {
  return `Не нашла такое место: ${target.shown}.`;
}

async function lookup(target, days) {
  const until = Date.now() + TIMEOUT;
  const place = await locate(target, until);
  if (!place) return { text: notFound(target) };
  const url = `${FORECAST}?latitude=${place.latitude}&longitude=${place.longitude}` +
    "&current=temperature_2m,apparent_temperature,weather_code,wind_speed_10m" +
    "&daily=weather_code,temperature_2m_max,temperature_2m_min,precipitation_probability_max" +
    "&timezone=auto&forecast_days=7&wind_speed_unit=ms";
  const forecast = await fetchForecast(url, until);
  return { text: describe(forecast, place.name, days), now: days === 0 ? forecast.current?.temperature_2m : undefined };
}

async function usualPlace(request) {
  const own = String(host.settings.get("place") ?? "").trim();
  if (own) return own;
  const panel = await host.bus.call("system/home", "weather-place").catch(() => null);
  if (panel) return panel;
  const zone = String(request?.timezone ?? "").split("/").pop();
  return zone ? zone.replace(/_/g, " ") : null;
}

function daysOf(when) {
  const text = normalize(when);
  if (text.includes("недел") || text === "week") return 7;
  if (text.includes("завтра") || text === "tomorrow") return 1;
  return 0;
}

// Фраза: «какая погода завтра в москве».
export async function forecast({ when, request }) {
  let target = placeIn(when ?? "");
  if (!target) {
    const usual = await usualPlace(request);
    if (!usual) return "Не знаю, для какого города узнать погоду.";
    target = givenPlace(usual);
  }
  const answer = await lookup(target, daysOf(when ?? ""));
  return answer.now === undefined ? answer.text : { say: answer.text, display: { temperature: answer.now } };
}

// Инструмент weather_now — «Мозгу» и утреннему шоу.
export async function weatherNow({ place, when, timezone }) {
  const name = String(place ?? "").trim() || (await usualPlace({ timezone }));
  if (!name) return "Место для погоды не задано.";
  return (await lookup(givenPlace(name), daysOf(when ?? "now"))).text;
}

// Виджет «Погода» на столе пульта (panel.js, docs/plans/desk.md): сейчас и три дня вперёд — данными, не словами.
// Прогноз держится 10 минут: стол перечитывает его с каждой переменой на шлюзе. Кэш помнит своё место: сменили город
// в плагине или в «Дом» → «Коннекторы» → «Погода» — виджет сразу берёт новый.
const PANEL_TTL_MS = 10 * 60 * 1000;
const WEEKDAYS = ["воскресенье", "понедельник", "вторник", "среда", "четверг", "пятница", "суббота"];

export async function panelWeather() {
  const { _err, ...data } = await panelData();
  return data;
}

// The same, with `_err` (a dictionary key and params) beside the Russian error text: the card words it itself.
async function panelData() {
  const until = Date.now() + TIMEOUT;
  const name = await usualPlace({});
  if (!name) return { place: null, error: "Место для погоды не задано: «Дом» → «Коннекторы» → «Погода».", _err: { key: "noPlace" } };
  const cached = host.kv.get("panel:weather:2");
  if (cached && cached.name === name && Date.now() - cached.at < PANEL_TTL_MS) return cached.data;
  const place = await locate(givenPlace(name), until);
  if (!place) return { place: name, error: `Не нашла такое место: ${name}.`, _err: { key: "notFound", params: { place: name } } };
  const url = `${FORECAST}?latitude=${place.latitude}&longitude=${place.longitude}` +
    "&current=temperature_2m,apparent_temperature,weather_code,wind_speed_10m,is_day" +
    "&daily=weather_code,temperature_2m_max,temperature_2m_min,precipitation_probability_max" +
    "&timezone=auto&forecast_days=4&wind_speed_unit=ms";
  const forecast = await fetchForecast(url, until);
  const now = forecast.current ?? {};
  const daily = forecast.daily ?? {};
  const days = (daily.time ?? []).slice(1, 4).map((date, index) => ({
    date,
    label: index === 0 ? "завтра" : WEEKDAYS[new Date(`${date}T12:00:00`).getDay()],
    max: Math.round(daily.temperature_2m_max[index + 1]),
    min: Math.round(daily.temperature_2m_min[index + 1]),
    text: describeCode(daily.weather_code[index + 1]),
    code: daily.weather_code[index + 1],
    rain: daily.precipitation_probability_max?.[index + 1] ?? null,
  }));
  const data = {
    place: place.name,
    now: {
      temperature: Math.round(now.temperature_2m),
      feels: now.apparent_temperature === undefined ? null : Math.round(now.apparent_temperature),
      text: describeCode(now.weather_code),
      code: now.weather_code,
      isDay: now.is_day === undefined ? null : now.is_day === 1,
      wind: now.wind_speed_10m === undefined ? null : Math.round(now.wind_speed_10m),
    },
    days,
  };
  host.kv.set("panel:weather:2", { at: Date.now(), name, data });
  return data;
}

const STORM = [95, 96, 99];
const SNOW = [71, 73, 75, 77, 85, 86];
const RAIN = [51, 53, 55, 56, 57, 61, 63, 65, 66, 67, 80, 81, 82];
const FOG = [45, 48];

// Значок дня в плитке.
function iconOf(code) {
  if (STORM.includes(code)) return "⛈";
  if (SNOW.includes(code)) return "❄";
  if (RAIN.includes(code)) return "🌧";
  if (FOG.includes(code)) return "🌫";
  if (code === 0) return "☀";
  if (code === 1 || code === 2) return "⛅";
  return "☁";
}

// Вид карточки по небу (docs/plugins/cards.md, «Вид»): сцена — что за окном, цвета — день или ночь, жара или мороз.
function skyLook(now) {
  const code = now.code ?? 3;
  const day = now.isDay !== false;
  if (STORM.includes(code)) return { scene: "storm", colors: ["#141a26", "#2c3748", "#46546a"] };
  if (SNOW.includes(code)) return { scene: "snow", colors: day ? ["#5a86c9", "#93bde9", "#cfe4fa"] : ["#16233f", "#2f4a73"] };
  if (RAIN.includes(code)) return { scene: "rain", colors: day ? ["#2d3e55", "#4b6987", "#6a8aa9"] : ["#101827", "#24344b"] };
  if (FOG.includes(code)) return { scene: "fog", colors: day ? ["#66727f", "#9ea9b6"] : ["#272e38", "#46505d"] };
  if (!day) return code <= 1 ? { scene: "stars", colors: ["#060a26", "#1c1950", "#3a2a73"] } : { scene: "clouds", colors: ["#0d152e", "#27314e"] };
  if (code <= 1) {
    if (now.temperature >= 28) return { scene: "sun", colors: ["#f08a4b", "#f4b860", "#8cc8f0"] };
    if (now.temperature <= -10) return { scene: "sun", colors: ["#3f6fbf", "#8dbbea", "#e3f0fd"] };
    return { scene: "sun", colors: ["#1f6fd6", "#58aef5", "#a6d8ff"] };
  }
  return { scene: "clouds", colors: code === 2 ? ["#3a74c4", "#7aaee4"] : ["#56687d", "#8c9fb4"] };
}

// Карточка на столе пульта (docs/plugins/cards.md): сейчас крупно, три дня плитками со значками, фон — живое небо
// за окном. Рисует пульт.
const T = {
  ru: {
    noPlace: "Место для погоды не задано: «Дом» → «Коннекторы» → «Погода».",
    notFound: "Не нашла такое место: {place}.",
    wind: "ветер {n} м/с",
    tomorrow: "завтра",
    d0: "воскресенье", d1: "понедельник", d2: "вторник", d3: "среда", d4: "четверг", d5: "пятница", d6: "суббота",
    c0: "ясно", c1: "преимущественно ясно", c2: "переменная облачность", c3: "пасмурно", c45: "туман", c51: "морось",
    c56: "ледяная морось", c61: "небольшой дождь", c63: "дождь", c65: "сильный дождь", c66: "ледяной дождь",
    c71: "небольшой снег", c73: "снег", c75: "сильный снег", c77: "снежная крупа", c80: "ливень", c82: "сильный ливень",
    c85: "снегопад", c95: "гроза", c96: "гроза с градом", cNone: "погода без описания",
  },
  en: {
    noPlace: "No place set for weather: Home → Connectors → Weather.",
    notFound: "Couldn't find that place: {place}.",
    wind: "wind {n} m/s",
    tomorrow: "tomorrow",
    d0: "Sunday", d1: "Monday", d2: "Tuesday", d3: "Wednesday", d4: "Thursday", d5: "Friday", d6: "Saturday",
    c0: "clear", c1: "mostly clear", c2: "partly cloudy", c3: "overcast", c45: "fog", c51: "drizzle",
    c56: "freezing drizzle", c61: "light rain", c63: "rain", c65: "heavy rain", c66: "freezing rain",
    c71: "light snow", c73: "snow", c75: "heavy snow", c77: "snow grains", c80: "showers", c82: "heavy showers",
    c85: "snow showers", c95: "thunderstorm", c96: "thunderstorm with hail", cNone: "no description",
  },
};
const CODE_KEYS = [
  [[0], "c0"], [[1], "c1"], [[2], "c2"], [[3], "c3"], [[45, 48], "c45"], [[51, 53, 55], "c51"], [[56, 57], "c56"],
  [[61], "c61"], [[63], "c63"], [[65], "c65"], [[66, 67], "c66"], [[71], "c71"], [[73], "c73"], [[75], "c75"],
  [[77], "c77"], [[80, 81], "c80"], [[82], "c82"], [[85, 86], "c85"], [[95], "c95"], [[96, 99], "c96"],
];
// A weather code in the home's language.
function codeText(code) {
  const found = CODE_KEYS.find(([codes]) => codes.includes(code));
  return host.i18n.t(T, found ? found[1] : "cNone");
}

export async function weatherCard() {
  // Без прогноза — спокойное облачное небо.
  const quiet = { scene: "clouds", colors: ["#3a74c4", "#7aaee4"] };
  let data;
  try {
    data = await panelData();
  } catch (error) {
    return { empty: String(error?.message ?? error), look: quiet };
  }
  if (data.error) return { empty: data._err ? host.i18n.t(T, data._err.key, data._err.params) : data.error, look: quiet };
  const now = data.now;
  const line = [codeText(now.code), now.wind !== null ? host.i18n.t(T, "wind", { n: now.wind }) : null, data.place].filter(Boolean).join(" · ");
  return {
    big: `${signed(now.temperature)}°`,
    cap: line,
    tiles: data.days.map((day, index) => ({ label: index === 0 ? host.i18n.t(T, "tomorrow") : host.i18n.t(T, `d${new Date(`${day.date}T12:00:00`).getDay()}`), value: `${iconOf(day.code)} ${signed(day.max)}°` })),
    look: skyLook(now),
    say: `${data.place}: ${signed(now.temperature)}°, ${now.text}.`,
  };
}
