// Календарь по ссылке iCal (.ics): Google, Яндекс, iCloud дают её без входа в аккаунт. Голосом — «что у меня сегодня»,
// «Мозгу» — calendar_events, столу пульта — panelEvents (docs/plans/desk.md).
//
// Лента разбирается раз в 15 минут, а в kv ложатся только вхождения ближайших дней, коротко: значение kv — не больше
// 64 КБ, а лента Google со всеми прошлыми событиями давно больше. Повторы (RRULE): каждый день, неделю, месяц, год;
// INTERVAL, COUNT, UNTIL, WKST, BYDAY (у месяца и года — и «2TU», «-1FR»), BYMONTHDAY, BYMONTH, BYSETPOS. Правило с
// чем-то ещё (BYWEEKNO, BYYEARDAY, BYHOUR…) не разворачивается: остаётся первое вхождение, а не выдуманные даты.
// Исключения — EXDATE и экземпляры, перенесённые или отменённые отдельно (RECURRENCE-ID); RDATE — добавочные даты.
// Время с TZID переводится по VTIMEZONE ленты. «Сегодня» и часы голосом — по поясу колонки (request.timezone),
// «Мозгу» — по поясу, который он назвал, виджету — по поясу календаря (X-WR-TIMEZONE).

const MINUTE_MS = 60 * 1000;
const HOUR_MS = 60 * MINUTE_MS;
const DAY_MS = 24 * HOUR_MS;
const TTL_MS = 15 * MINUTE_MS;
// Календарь не отвечает — ответ по кэшу, если ему не больше 12 часов: вхождений в нём на две недели вперёд.
const STALE_MS = 12 * HOUR_MS;
// Вызов голоса, «Мозга» и пульта ограничен 2 с вместе с разбором ленты.
const TIMEOUT = 1700;
// Кэш: вхождения, что задевают [−25 часов; +15 дней) от разбора, — не больше 300 и не больше 60 КБ JSON (kv берёт до
// 64 КБ). Этого хватает «сегодня» в любом поясе и двум неделям «Мозга», в том числе по кэшу на 12 часов старше.
const CACHE = "cache";
const BEHIND_MS = 25 * HOUR_MS;
const AHEAD_MS = 15 * DAY_MS;
const KEEP = 300;
const KEEP_BYTES = 60 * 1024;
// Название и место в кэше — не длиннее стольких символов (shorten).
const TITLE = 80;
const PLACE = 60;
// Повтор, у которого вхождения не находятся столько периодов подряд, дальше не разворачивается.
const STEPS = 50000;

// Беда с лентой, которую можно сказать человеку; lasting — ссылку надо поправить, кэш тут не поможет.
function trouble(text, lasting = false) {
  const error = new Error(text);
  error.calendar = true;
  error.lasting = lasting;
  return error;
}

function message(error) {
  if (error?.calendar) return error.message;
  console.warn(`календарь: ${error?.message ?? error}`);
  return "Не получилось прочитать календарь.";
}

function link() {
  const raw = String(host.settings.get("ics") ?? "").trim();
  return raw.replace(/^webcal:\/\//i, "https://");
}

// Ссылка секретная (в ней ключ календаря): кэш помнит не её, а отпечаток — сменили ссылку, кэш не подходит.
function fingerprint(url) {
  let hash = 0x811c9dc5;
  for (let index = 0; index < url.length; index += 1) {
    hash ^= url.charCodeAt(index);
    hash = Math.imul(hash, 0x01000193) >>> 0;
  }
  return `${url.length}:${hash.toString(16)}`;
}

// Лента целиком, только если это и правда календарь.
async function download(url, timeoutMs) {
  let answer;
  try {
    answer = await host.http.fetch(url, { timeoutMs });
  } catch (error) {
    const text = String(error?.message ?? error);
    if (/больше \d+ КБ/.test(text)) throw trouble("Лента календаря больше 1 МБ — столько шлюз не берёт.", true);
    if (/не разрешён|не читается|только http|нет хоста|адрес этого компьютера/.test(text)) {
      throw trouble("По этой ссылке календарь не открыть: подходят ссылки iCal от Google, Яндекса, iCloud и Outlook.", true);
    }
    throw trouble("Календарь сейчас не открывается — попробуйте чуть позже.");
  }
  if (!answer.ok) {
    if (answer.status >= 400 && answer.status < 500) {
      throw trouble(`Ссылка на календарь не работает (ответ ${answer.status}) — проверьте её в настройках приложения «Календарь».`, true);
    }
    throw trouble(`Сервер календаря ответил ошибкой ${answer.status} — попробуйте позже.`);
  }
  const text = await answer.text();
  if (!/^\s*BEGIN:VCALENDAR/i.test(text.slice(0, 200))) {
    throw trouble("По ссылке не календарь iCal (.ics) — проверьте её в настройках приложения «Календарь».", true);
  }
  return text;
}

// Вхождения из кэша или из свежей ленты; null — ссылки нет.
async function source(timeoutMs) {
  const url = link();
  if (!url) return null;
  let cached = null;
  try {
    cached = host.kv.get(CACHE);
  } catch (error) {
    console.warn(`кэш календаря не читается: ${error.message}`);
  }
  if (cached?.v !== 2 || cached.link !== fingerprint(url)) cached = null;
  if (cached && Date.now() - cached.at < TTL_MS) return cached;
  let text;
  try {
    text = await download(url, timeoutMs);
  } catch (error) {
    if (cached && !error.lasting && Date.now() - cached.at < STALE_MS) return cached;
    throw error;
  }
  const fresh = build(url, text, Date.now());
  try {
    host.kv.set(CACHE, fresh);
    // Прежняя версия держала здесь сырую ленту.
    host.kv.delete("ics");
  } catch (error) {
    console.warn(`кэш календаря не записался: ${error.message} — работаю без него`);
  }
  return fresh;
}

// Строки iCal.

function unfold(block) {
  return block.replace(/\r?\n[ \t]/g, "").split(/\r?\n/);
}

// «Имя;параметры» и значение: двоеточие в кавычках параметра — не граница.
function cut(line) {
  let quoted = false;
  for (let index = 0; index < line.length; index += 1) {
    const code = line.charCodeAt(index);
    if (code === 34) quoted = !quoted;
    else if (code === 58 && !quoted) return [line.slice(0, index), line.slice(index + 1)];
  }
  return null;
}

function paramsOf(head) {
  const params = {};
  for (const part of head.match(/;[^;=]+=(?:"[^"]*"|[^;]*)/g) ?? []) {
    const eq = part.indexOf("=");
    params[part.slice(1, eq).trim().toUpperCase()] = part.slice(eq + 1).replace(/^"|"$/g, "");
  }
  return params;
}

function unescape(value) {
  return value
    .replace(/\\([\\;,nN])/g, (_, char) => (char === "n" || char === "N" ? " " : char))
    .replace(/\s+/g, " ")
    .trim();
}

// Компоненты верхнего уровня: текст между BEGIN:<name> и END:<name>.
function each(text, name, fn) {
  const open = `\nBEGIN:${name}`;
  const close = `\nEND:${name}`;
  let at = text.indexOf(open);
  while (at >= 0) {
    const end = text.indexOf(close, at + open.length);
    if (end < 0) return;
    fn(text.slice(at + open.length, end));
    at = text.indexOf(open, end + close.length);
  }
}

// Время «по стене» — местные часы, записанные как UTC: так дни и месяцы считаются без перехода на летнее время, а в
// настоящий момент переводит пояс.

// 20261110T090000Z — UTC; 20261110T090000 — по поясу TZID или «плавающее»; 20261110 — дата, весь день.
function stamp(value, params) {
  const found = /^(\d{4})(\d{2})(\d{2})(?:T(\d{2})(\d{2})(\d{2})(Z)?)?$/i.exec(String(value).trim());
  if (!found) return null;
  const [, y, m, d, hh, mm, ss, z] = found;
  const date = hh === undefined || String(params.VALUE ?? "").toUpperCase() === "DATE";
  const wall = date ? Date.UTC(+y, +m - 1, +d) : Date.UTC(+y, +m - 1, +d, +hh, +mm, +ss);
  return { wall, date, utc: !date && Boolean(z), tzid: !date && !z ? params.TZID ?? null : null };
}

// P1D, PT1H30M, P2W — длительность в мс.
function durationOf(value) {
  const found = /^([+-])?P(?:(\d+)W)?(?:(\d+)D)?(?:T(?:(\d+)H)?(?:(\d+)M)?(?:(\d+)S)?)?$/i.exec(String(value).trim());
  if (!found) return null;
  const [, sign, w, d, h, m, s] = found;
  const total = ((+w || 0) * 7 + (+d || 0)) * DAY_MS + (+h || 0) * HOUR_MS + (+m || 0) * MINUTE_MS + (+s || 0) * 1000;
  return sign === "-" ? 0 : total;
}

// +0700, -0330 — сдвиг пояса в мс.
function offsetOf(value) {
  const found = /^([+-])(\d{2})(\d{2})(\d{2})?$/.exec(String(value).trim());
  if (!found) return null;
  const total = (+found[2] * 3600 + +found[3] * 60 + (+found[4] || 0)) * 1000;
  return found[1] === "-" ? -total : total;
}

const floorDay = (wall) => Math.floor(wall / DAY_MS) * DAY_MS;

// Пояса: offsetAt(t) — сдвиг от UTC в момент t.

const UTC_ZONE = { offsetAt: () => 0 };
// Пояс процесса шлюза — когда другого не узнать.
const PROCESS_ZONE = { offsetAt: (t) => -new Date(t).getTimezoneOffset() * MINUTE_MS };

// Пояса без перехода на летнее время — на случай, когда в ленте нет VTIMEZONE пояса колонки: часы от UTC.
const FIXED = {
  UTC: 0, "Etc/UTC": 0, GMT: 0, "Etc/GMT": 0,
  "Europe/Kaliningrad": 2,
  "Europe/Moscow": 3, "Europe/Simferopol": 3, "Europe/Kirov": 3, "Europe/Volgograd": 3, "Europe/Minsk": 3,
  "Europe/Istanbul": 3,
  "Europe/Samara": 4, "Europe/Ulyanovsk": 4, "Europe/Saratov": 4, "Europe/Astrakhan": 4, "Asia/Tbilisi": 4,
  "Asia/Yerevan": 4, "Asia/Baku": 4, "Asia/Dubai": 4,
  "Asia/Yekaterinburg": 5, "Asia/Tashkent": 5, "Asia/Samarkand": 5, "Asia/Dushanbe": 5, "Asia/Ashgabat": 5,
  "Asia/Almaty": 5, "Asia/Qostanay": 5, "Asia/Aqtobe": 5, "Asia/Aqtau": 5, "Asia/Atyrau": 5, "Asia/Oral": 5,
  "Asia/Qyzylorda": 5,
  "Asia/Omsk": 6, "Asia/Bishkek": 6,
  "Asia/Novosibirsk": 7, "Asia/Barnaul": 7, "Asia/Tomsk": 7, "Asia/Novokuznetsk": 7, "Asia/Krasnoyarsk": 7,
  "Asia/Bangkok": 7, "Asia/Ho_Chi_Minh": 7,
  "Asia/Irkutsk": 8, "Asia/Shanghai": 8, "Asia/Hong_Kong": 8, "Asia/Singapore": 8,
  "Asia/Chita": 9, "Asia/Yakutsk": 9, "Asia/Khandyga": 9, "Asia/Tokyo": 9, "Asia/Seoul": 9,
  "Asia/Vladivostok": 10, "Asia/Ust-Nera": 10,
  "Asia/Magadan": 11, "Asia/Sakhalin": 11, "Asia/Srednekolymsk": 11,
  "Asia/Kamchatka": 12, "Asia/Anadyr": 12,
};

function fixedZone(name) {
  const text = String(name ?? "").trim();
  if (!text) return null;
  if (FIXED[text] !== undefined) return { offsetAt: () => FIXED[text] * HOUR_MS };
  const etc = /^Etc\/GMT([+-])(\d{1,2})$/.exec(text);
  if (etc) return { offsetAt: () => (etc[1] === "+" ? -1 : 1) * +etc[2] * HOUR_MS };
  const plain = /^(?:UTC|GMT)?\s*([+-])(\d{1,2})(?::?(\d{2}))?$/.exec(text);
  if (plain) return { offsetAt: () => (plain[1] === "-" ? -1 : 1) * (+plain[2] * HOUR_MS + (+plain[3] || 0) * MINUTE_MS) };
  return null;
}

// Сдвиги, записанные в кэш: [момент, сдвиг с него] по порядку.
function tableZone(entries) {
  return {
    offsetAt(t) {
      let offset = entries[0][1];
      for (const [at, to] of entries) {
        if (at > t) break;
        offset = to;
      }
      return offset;
    },
  };
}

// Начала одного периода VTIMEZONE (STANDARD, DAYLIGHT) — моменты в [lo, hi): DTSTART, RDATE и повтор. Время в них —
// местное по сдвигу «до» (TZOFFSETFROM).
function onsets(one, lo, hi) {
  const found = new Set();
  const add = (wall) => {
    const at = wall - one.from;
    if (at >= lo && at < hi) found.add(at);
  };
  add(one.start);
  for (const wall of one.rdates) add(wall);
  if (one.rule) for (const wall of expand(one.rule, one.start, lo + one.from, hi + one.from, one.until)) add(wall);
  return [...found];
}

// Последнее начало периода не позже t или null. Повтор, кончившийся давно (UNTIL), ищется у своего конца; с COUNT —
// с самого начала.
function latestOnset(one, t) {
  let best = null;
  const consider = (at) => {
    if (at <= t && (best === null || at > best)) best = at;
  };
  consider(one.start - one.from);
  for (const wall of one.rdates) consider(wall - one.from);
  if (one.rule) {
    const edge = Math.min(t, one.until - one.from);
    const lo = one.rule.count === Infinity ? edge - 400 * DAY_MS : -Infinity;
    for (const at of onsets(one, lo, edge + 1)) consider(at);
  }
  return best;
}

// Пояс по VTIMEZONE: сдвиг — у последнего начавшегося периода.
function ruleZone(observances) {
  const years = new Map();
  const latest = (t) => {
    let best = null;
    for (const one of observances) {
      const at = latestOnset(one, t);
      if (at !== null && (best === null || at > best.at)) best = { at, to: one.to };
    }
    if (best) return best.to;
    // Раньше всех периодов — сдвиг «до» самого раннего.
    let earliest = observances[0];
    for (const one of observances) if (one.start < earliest.start) earliest = one;
    return earliest.from;
  };
  const table = (year) => {
    let entry = years.get(year);
    if (entry) return entry;
    const lo = Date.UTC(year, 0, 1);
    const hi = Date.UTC(year + 1, 0, 1);
    const changes = [];
    for (const one of observances) for (const at of onsets(one, lo, hi)) changes.push([at, one.to]);
    changes.sort((a, b) => a[0] - b[0]);
    entry = { base: latest(lo - 1), changes };
    years.set(year, entry);
    return entry;
  };
  return {
    offsetAt(t) {
      const { base, changes } = table(new Date(t).getUTCFullYear());
      let offset = base;
      for (const [at, to] of changes) {
        if (at > t) break;
        offset = to;
      }
      return offset;
    },
    // Для кэша: сдвиг в начале окна и перемены внутри него.
    span(from, to) {
      const entries = [[from, this.offsetAt(from)]];
      for (let year = new Date(from).getUTCFullYear(); year <= new Date(to).getUTCFullYear(); year += 1) {
        for (const change of table(year).changes) if (change[0] > from && change[0] <= to) entries.push(change);
      }
      return entries;
    },
  };
}

// Местные часы → момент. В «дыре» перехода на летнее время — любой из двух.
function toUtc(zone, wall) {
  const first = zone.offsetAt(wall);
  const guess = wall - first;
  const second = zone.offsetAt(guess);
  if (second === first) return guess;
  const other = wall - second;
  return zone.offsetAt(other) === second ? other : guess;
}

function toWall(zone, at) {
  return at + zone.offsetAt(at);
}

// Повторы.

const WEEKDAYS = ["SU", "MO", "TU", "WE", "TH", "FR", "SA"];
const RULE_PARTS = new Set(["FREQ", "INTERVAL", "COUNT", "UNTIL", "BYDAY", "BYMONTHDAY", "BYMONTH", "BYSETPOS", "WKST"]);

// RRULE по частям; null — правило, которое здесь не разворачивается.
function ruleOf(text) {
  const parts = {};
  for (const part of String(text).split(";")) {
    const eq = part.indexOf("=");
    if (eq > 0) parts[part.slice(0, eq).trim().toUpperCase()] = part.slice(eq + 1).trim().toUpperCase();
  }
  const freq = parts.FREQ;
  if (!["DAILY", "WEEKLY", "MONTHLY", "YEARLY"].includes(freq)) return null;
  if (Object.keys(parts).some((key) => !RULE_PARTS.has(key))) return null;
  const numbers = (value, low, high) => {
    if (value === undefined) return null;
    const list = value.split(",").map((one) => (/^[+-]?\d+$/.test(one) ? Number(one) : NaN));
    return list.every((one) => one >= low && one <= high && one !== 0) ? list : undefined;
  };
  const interval = parts.INTERVAL === undefined ? 1 : /^\d+$/.test(parts.INTERVAL) ? Number(parts.INTERVAL) : 0;
  const count = parts.COUNT === undefined ? Infinity : /^\d+$/.test(parts.COUNT) ? Number(parts.COUNT) : 0;
  const byMonthDay = numbers(parts.BYMONTHDAY, -31, 31);
  const byMonth = numbers(parts.BYMONTH, 1, 12);
  const bySetPos = numbers(parts.BYSETPOS, -366, 366);
  const wkst = parts.WKST === undefined ? 1 : WEEKDAYS.indexOf(parts.WKST);
  let byDay = null;
  if (parts.BYDAY !== undefined) {
    byDay = parts.BYDAY.split(",").map((one) => {
      const found = /^([+-]?\d{1,2})?(SU|MO|TU|WE|TH|FR|SA)$/.exec(one);
      return found ? { n: found[1] ? Number(found[1]) : 0, day: WEEKDAYS.indexOf(found[2]) } : null;
    });
    if (byDay.some((one) => !one || Math.abs(one.n) > 53)) return null;
  }
  if (interval < 1 || count < 1 || wkst < 0) return null;
  if (byMonthDay === undefined || byMonth === undefined || bySetPos === undefined) return null;
  // Чего здесь нет: n-й день недели у дня и недели, число месяца у недели, дни недели года без месяца.
  const numbered = byDay?.some((one) => one.n !== 0);
  if ((freq === "DAILY" || freq === "WEEKLY") && numbered) return null;
  if (freq === "WEEKLY" && byMonthDay) return null;
  if (freq === "YEARLY" && byDay && !byMonth) return null;
  return { freq, interval, count, until: parts.UNTIL ?? null, byDay, byMonthDay, byMonth, bySetPos, wkst };
}

const daysIn = (year, month) => new Date(Date.UTC(year, month + 1, 0)).getUTCDate();

// Дни месяца по BYMONTHDAY и BYDAY (вместе — пересечение); без них — число DTSTART, если оно в месяце есть.
function monthDays(rule, year, month, startDate) {
  const length = daysIn(year, month);
  let days = null;
  if (rule.byMonthDay) days = rule.byMonthDay.map((one) => (one > 0 ? one : length + 1 + one)).filter((one) => one >= 1 && one <= length);
  if (rule.byDay) {
    const first = new Date(Date.UTC(year, month, 1)).getUTCDay();
    const weekdays = [];
    for (const { n, day } of rule.byDay) {
      const all = [];
      for (let date = 1 + ((day - first + 7) % 7); date <= length; date += 7) all.push(date);
      if (n === 0) weekdays.push(...all);
      else {
        const pick = n > 0 ? all[n - 1] : all[all.length + n];
        if (pick) weekdays.push(pick);
      }
    }
    days = days ? days.filter((one) => weekdays.includes(one)) : weekdays;
  }
  if (!days) days = startDate <= length ? [startDate] : [];
  return days.map((one) => Date.UTC(year, month, one));
}

// Начала вхождений повтора («по стене») в [lo, hi): DTSTART — всегда первое, оно же первое в COUNT.
function expand(rule, startWall, lo, hi, untilWall) {
  const found = [];
  if (startWall >= lo && startWall < hi && startWall <= untilWall) found.push(startWall);
  const time = startWall - floorDay(startWall);
  const startDay = startWall - time;
  const first = new Date(startDay);
  const year0 = first.getUTCFullYear();
  const month0 = first.getUTCMonth();
  const date0 = first.getUTCDate();
  const weekday0 = first.getUTCDay();
  const weekStart0 = startDay - ((weekday0 - rule.wkst + 7) % 7) * DAY_MS;
  const step = rule.interval;
  // Без COUNT — сразу к периоду незадолго до окна: ежедневному повтору с 2020 года не нужны тысячи шагов.
  let k = 0;
  if (rule.count === Infinity && lo > startWall) {
    const at = new Date(lo);
    const months = (at.getUTCFullYear() - year0) * 12 + at.getUTCMonth() - month0;
    if (rule.freq === "DAILY") k = Math.floor((lo - startDay) / (step * DAY_MS));
    else if (rule.freq === "WEEKLY") k = Math.floor((lo - weekStart0) / (step * 7 * DAY_MS));
    else if (rule.freq === "MONTHLY") k = Math.floor(months / step);
    else k = Math.floor(Math.floor(months / 12) / step);
    k = Math.max(0, k - 1);
  }
  let made = 1;
  for (let guard = 0; guard < STEPS; guard += 1, k += 1) {
    let begin;
    let days;
    if (rule.freq === "DAILY") {
      begin = startDay + k * step * DAY_MS;
      const date = new Date(begin);
      const length = daysIn(date.getUTCFullYear(), date.getUTCMonth());
      const fits =
        (!rule.byMonth || rule.byMonth.includes(date.getUTCMonth() + 1)) &&
        (!rule.byDay || rule.byDay.some((one) => one.day === date.getUTCDay())) &&
        (!rule.byMonthDay || rule.byMonthDay.some((one) => (one > 0 ? one : length + 1 + one) === date.getUTCDate()));
      days = fits ? [begin] : [];
    } else if (rule.freq === "WEEKLY") {
      begin = weekStart0 + k * step * 7 * DAY_MS;
      const weekdays = rule.byDay ? rule.byDay.map((one) => one.day) : [weekday0];
      days = [...new Set(weekdays)]
        .map((day) => begin + ((day - rule.wkst + 7) % 7) * DAY_MS)
        .filter((day) => !rule.byMonth || rule.byMonth.includes(new Date(day).getUTCMonth() + 1));
    } else if (rule.freq === "MONTHLY") {
      const total = month0 + k * step;
      const year = year0 + Math.floor(total / 12);
      const month = total % 12;
      begin = Date.UTC(year, month, 1);
      days = !rule.byMonth || rule.byMonth.includes(month + 1) ? monthDays(rule, year, month, date0) : [];
    } else {
      const year = year0 + k * step;
      begin = Date.UTC(year, 0, 1);
      const months = rule.byMonth ?? (rule.byMonthDay ? [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12] : [month0 + 1]);
      days = months.flatMap((month) => monthDays(rule, year, month - 1, date0));
    }
    if (begin + time >= hi || begin > untilWall) break;
    days = [...new Set(days)].sort((a, b) => a - b);
    if (rule.bySetPos) {
      days = [...new Set(rule.bySetPos.map((pos) => days[pos > 0 ? pos - 1 : days.length + pos]).filter((day) => day !== undefined))].sort((a, b) => a - b);
    }
    for (const day of days) {
      const wall = day + time;
      if (wall <= startWall) continue;
      if (wall > untilWall) return found;
      made += 1;
      if (made > rule.count) return found;
      if (wall >= hi) return found;
      if (wall >= lo) found.push(wall);
    }
  }
  return found;
}

// Лента.

function readZone(block) {
  let tzid = null;
  let depth = 0;
  let current = null;
  const observances = [];
  for (const line of unfold(block)) {
    if (/^BEGIN:/i.test(line)) {
      depth += 1;
      const kind = line.slice(6).trim().toUpperCase();
      current = depth === 1 && (kind === "STANDARD" || kind === "DAYLIGHT") ? { rdates: [] } : null;
      continue;
    }
    if (/^END:/i.test(line)) {
      if (depth === 1 && current && current.start !== undefined && typeof current.from === "number" && typeof current.to === "number") {
        // UNTIL правила пояса — в UTC: «по стене» — по сдвигу «до».
        const until = current.rule?.until ? stamp(current.rule.until, {}) : null;
        current.until = !until ? Infinity : until.date ? until.wall + DAY_MS - 1 : until.wall + (until.utc ? current.from : 0);
        observances.push(current);
      }
      depth -= 1;
      current = null;
      continue;
    }
    const parts = cut(line);
    if (!parts) continue;
    const [head, value] = parts;
    const name = head.split(";")[0].trim().toUpperCase();
    if (depth === 0 && name === "TZID") tzid = value.trim();
    if (depth !== 1 || !current) continue;
    if (name === "DTSTART") {
      const at = stamp(value, paramsOf(head));
      if (at) current.start = at.wall;
    } else if (name === "TZOFFSETFROM") current.from = offsetOf(value);
    else if (name === "TZOFFSETTO") current.to = offsetOf(value);
    // Правило, которое здесь не разворачивается, — без повторов: остаётся начало периода.
    else if (name === "RRULE") current.rule = ruleOf(value);
    else if (name === "RDATE") {
      for (const one of value.split(",")) {
        const at = stamp(one, paramsOf(head));
        if (at) current.rdates.push(at.wall);
      }
    }
  }
  return tzid && observances.length ? { tzid, zone: ruleZone(observances) } : null;
}

// Свойства события — только его уровня: SUMMARY напоминания (VALARM) событие не переименовывает.
function readEvent(block) {
  const event = { exdates: [], rdates: [] };
  let depth = 0;
  for (const line of unfold(block)) {
    if (/^BEGIN:/i.test(line)) {
      depth += 1;
      continue;
    }
    if (/^END:/i.test(line)) {
      depth -= 1;
      continue;
    }
    if (depth > 0) continue;
    const parts = cut(line);
    if (!parts) continue;
    const [head, value] = parts;
    const semi = head.indexOf(";");
    const name = (semi < 0 ? head : head.slice(0, semi)).trim().toUpperCase();
    if (name === "SUMMARY") event.title = unescape(value);
    else if (name === "LOCATION") event.place = unescape(value);
    else if (name === "UID") event.uid = value.trim();
    else if (name === "DTSTART") event.start = stamp(value, paramsOf(head));
    else if (name === "DTEND") event.end = stamp(value, paramsOf(head));
    else if (name === "DURATION") event.duration = durationOf(value);
    else if (name === "RRULE") event.rule = event.rule ?? value.trim();
    else if (name === "RDATE" || name === "EXDATE") {
      const params = paramsOf(head);
      for (const one of value.split(",")) {
        const at = stamp(one, params);
        if (at) (name === "RDATE" ? event.rdates : event.exdates).push(at);
      }
    } else if (name === "RECURRENCE-ID") {
      const params = paramsOf(head);
      event.recurrence = stamp(value, params);
      event.future = String(params.RANGE ?? "").toUpperCase() === "THISANDFUTURE";
    } else if (name === "STATUS") event.cancelled = value.trim().toUpperCase() === "CANCELLED";
  }
  return event.start ? event : null;
}

// События ленты; разовые, кончившиеся раньше дня keepDay («ГГГГММДД»), пропускаются, не разбираясь.
function parse(text, keepDay) {
  const firstEvent = text.indexOf("BEGIN:VEVENT");
  const header = firstEvent < 0 ? text : text.slice(0, firstEvent);
  const named = /(?:^|\n)X-WR-TIMEZONE[^:\r\n]*:([^\r\n]+)/i.exec(header)?.[1]?.trim() || null;
  const zones = new Map();
  each(text, "VTIMEZONE", (block) => {
    const found = readZone(block);
    if (found && !zones.has(found.tzid)) zones.set(found.tzid, found.zone);
  });
  const events = [];
  each(text, "VEVENT", (block) => {
    if (!/\n(?:RRULE|RDATE|RECURRENCE-ID)[;:]/.test(block) && !/\nDURATION[;:]/.test(block)) {
      const start = /\nDTSTART[^:\r\n]*:(\d{8})/.exec(block)?.[1];
      const end = /\nDTEND[^:\r\n]*:(\d{8})/.exec(block)?.[1];
      const last = end && start && end > start ? end : start;
      if (last && last < keepDay) return;
    }
    const event = readEvent(block);
    if (event) events.push(event);
  });
  // Пояс календаря: X-WR-TIMEZONE, а без него — единственный VTIMEZONE ленты.
  const calendar = named ?? (zones.size === 1 ? [...zones.keys()][0] : null);
  return { calendar, zones, events };
}

function zoneByName(zones, name) {
  const text = String(name ?? "").trim();
  if (!text) return null;
  if (zones.has(text)) return zones.get(text);
  const fixed = fixedZone(text);
  if (fixed) return fixed;
  // «/mozilla.org/20050126_1/Europe/Moscow» и подобные — по последним двум частям.
  const tail = /[A-Za-z_]+\/[A-Za-z_+-]+$/.exec(text)?.[0];
  return tail && tail !== text ? zoneByName(zones, tail) : null;
}

const dayStamp = (at) => new Date(at).toISOString().slice(0, 10).replace(/-/g, "");

// Кэш из ленты: вхождения, что задевают [now − 25 часов; now + 15 дней), и пояса ленты на это время.
function build(url, text, now) {
  const from = now - BEHIND_MS;
  const to = now + AHEAD_MS;
  const feed = parse(text, dayStamp(from - 2 * DAY_MS));
  const fallback = zoneByName(feed.zones, feed.calendar) ?? PROCESS_ZONE;
  const zoneOf = (at) => (at.utc ? UTC_ZONE : at.tzid ? zoneByName(feed.zones, at.tzid) ?? fallback : fallback);
  const instantOf = (at) => (at.utc ? at.wall : toUtc(zoneOf(at), at.wall));
  // Перенесённые и отменённые экземпляры — исключения своего повтора (по UID).
  const masters = new Map();
  for (const event of feed.events) if (!event.recurrence && event.uid) masters.set(event.uid, event);
  for (const event of feed.events) {
    const master = event.recurrence ? masters.get(event.uid) : null;
    if (!master) continue;
    (master.moved ??= []).push(event);
    event.title ??= master.title;
    event.place ??= master.place;
  }
  const list = [];
  let unread = 0;
  for (const event of feed.events) {
    if (event.cancelled) continue;
    if (collect(event, { zoneOf, instantOf, fallback }, from, to, list)) unread += 1;
  }
  if (unread) console.log(`календарь: ${unread} повтор(ов) с правилом, которое не разворачивается, — только первое вхождение`);
  const sortKey = (one) => (one.d === undefined ? one.s : one.d * DAY_MS);
  list.sort((a, b) => sortKey(a) - sortKey(b));
  const zones = {};
  for (const [name, zone] of feed.zones) {
    if (Object.keys(zones).length >= 8) break;
    zones[name] = zone.span(from - DAY_MS, to + DAY_MS);
  }
  const cache = { v: 2, link: fingerprint(url), at: now, calendar: feed.calendar, zones, until: null, events: [] };
  let bytes = utf8(JSON.stringify(cache));
  for (const one of list) {
    const size = utf8(JSON.stringify(one)) + 1;
    if (cache.events.length >= KEEP || bytes + size > KEEP_BYTES) {
      // Дальше этого кэш неполон: вопрос о днях позже получит то, что в нём есть.
      cache.until = sortKey(one);
      break;
    }
    cache.events.push(one);
    bytes += size;
  }
  return cache;
}

// Текст не длиннее max символов. Режется по символам, а не по половинкам UTF-16: эмодзи на границе не оставляет
// одинокую половинку, а с ней JSON у шлюза не читается — kv.set и ответ функции отказывают.
function shorten(text, max) {
  if (text.length <= max) return text;
  // В max символах — не больше 2·max половинок: дальше текст не нужен.
  return Array.from(text.slice(0, 2 * max)).slice(0, max).join("");
}

function utf8(text) {
  let bytes = 0;
  for (let index = 0; index < text.length; index += 1) {
    const code = text.charCodeAt(index);
    if (code < 0x80) bytes += 1;
    else if (code < 0x800) bytes += 2;
    else if (code >= 0xd800 && code < 0xdc00) {
      bytes += 4;
      index += 1;
    } else bytes += 3;
  }
  return bytes;
}

// Вхождения одного события в [from, to) — в список кэша. true — правило повтора не развернулось. fallback — пояс
// календаря: по нему дата события на весь день сравнивается с исключением в UTC.
function collect(event, { zoneOf, instantOf, fallback }, from, to, out) {
  const start = event.start;
  const allDay = start.date;
  const zone = allDay ? null : zoneOf(start);
  const title = shorten(event.title || "Без названия", TITLE).trim();
  const place = event.place ? shorten(event.place, PLACE).trim() : "";
  let days = 1;
  let length = 0;
  if (allDay) {
    if (event.end?.date) days = Math.max(1, Math.round((event.end.wall - start.wall) / DAY_MS));
    else if (event.duration) days = Math.max(1, Math.round(event.duration / DAY_MS));
  } else if (event.end && !event.end.date) length = Math.max(0, instantOf(event.end) - toUtc(zone, start.wall));
  else if (event.duration) length = event.duration;
  // Исключения — «по стене» события: точное время или целый день.
  const skipWalls = new Set();
  const skipDays = new Set();
  let cutoff = Infinity;
  const wallOf = (at) => (at.date ? at.wall : allDay ? (at.utc ? toWall(fallback, at.wall) : at.wall) : toWall(zone, instantOf(at)));
  for (const at of event.exdates) {
    if (at.date || allDay) skipDays.add(floorDay(wallOf(at)));
    else skipWalls.add(wallOf(at));
  }
  for (const moved of event.moved ?? []) {
    const at = moved.recurrence;
    if (!at) continue;
    const wall = wallOf(at);
    if (moved.future) cutoff = Math.min(cutoff, at.date || allDay ? floorDay(wall) : wall);
    else if (at.date || allDay) skipDays.add(floorDay(wall));
    else skipWalls.add(wall);
  }
  const skipped = (wall) => wall >= cutoff || skipWalls.has(wall) || skipDays.has(floorDay(wall));
  const span = allDay ? days * DAY_MS : length;
  // Окно «по стене» — с запасом на сдвиг пояса; точно отбирается по моментам ниже.
  const lo = (allDay ? from : toWall(zone, from)) - span - DAY_MS;
  const hi = (allDay ? to : toWall(zone, to)) + DAY_MS;
  let walls;
  let unread = false;
  const rule = event.rule ? ruleOf(event.rule) : null;
  if (rule) {
    const until = rule.until ? stamp(rule.until, {}) : null;
    let untilWall = Infinity;
    if (until?.date) untilWall = until.wall + DAY_MS - 1;
    else if (until) untilWall = until.utc ? toWall(allDay ? fallback : zone, until.wall) : until.wall;
    walls = expand(rule, start.wall, lo, hi, untilWall);
  } else {
    unread = Boolean(event.rule);
    walls = start.wall >= lo && start.wall < hi ? [start.wall] : [];
  }
  for (const at of event.rdates) {
    const wall = at.date ? at.wall : allDay ? floorDay(at.wall) : toWall(zone, instantOf(at));
    if (wall >= lo && wall < hi) walls.push(wall);
  }
  for (const wall of [...new Set(walls)]) {
    if (skipped(wall)) continue;
    if (allDay) {
      // Дата без пояса: момент — у слушателя. Здесь — с запасом в полсуток в обе стороны.
      if (wall - 14 * HOUR_MS < to && wall + days * DAY_MS + 14 * HOUR_MS > from) {
        out.push(place ? { t: title, p: place, d: wall / DAY_MS, n: days } : { t: title, d: wall / DAY_MS, n: days });
      }
      continue;
    }
    const s = toUtc(zone, wall);
    const e = s + length;
    if (s < to && (e > from || s >= from)) out.push(place ? { t: title, p: place, s, e } : { t: title, s, e });
  }
  return unread;
}

// Слушатель: его пояс, «сегодня» и вхождения из кэша.

// Пояс по имени: из ленты (VTIMEZONE в кэше), из таблицы, пояс календаря, наконец — пояс шлюза.
function listener(cache, name) {
  const zones = new Map(Object.entries(cache.zones ?? {}).map(([key, entries]) => [key, tableZone(entries)]));
  const own = zoneByName(zones, name);
  if (own) return { zone: own, name: String(name).trim() };
  const calendar = zoneByName(zones, cache.calendar);
  if (calendar) return { zone: calendar, name: cache.calendar };
  return { zone: PROCESS_ZONE, name: null };
}

// Полночь сегодня «по стене» пояса.
function todayOf(zone) {
  return floorDay(toWall(zone, Date.now()));
}

// События, что задевают [from, to), по порядку; у события на весь день — день («по стене») и сколько дней.
function occurrences(cache, zone, from, to) {
  const end = cache.until ? Math.min(to, cache.until) : to;
  const found = [];
  for (const one of cache.events ?? []) {
    const event =
      one.d === undefined
        ? { title: one.t, place: one.p ?? null, start: one.s, end: one.e, allDay: false }
        : {
            title: one.t,
            place: one.p ?? null,
            start: toUtc(zone, one.d * DAY_MS),
            end: toUtc(zone, (one.d + one.n) * DAY_MS),
            allDay: true,
            day: one.d * DAY_MS,
            days: one.n,
          };
    if (event.start < end && (event.end > from || event.start >= from)) found.push(event);
  }
  return found.sort((a, b) => a.start - b.start || Number(b.allDay) - Number(a.allDay));
}

const two = (value) => String(value).padStart(2, "0");

function clockOf(zone, at) {
  const wall = new Date(toWall(zone, at));
  return `${two(wall.getUTCHours())}:${two(wall.getUTCMinutes())}`;
}

const isoDate = (wall) => new Date(wall).toISOString().slice(0, 10);

// 2026-11-11T18:30+07:00 — местное время со сдвигом.
function isoLocal(zone, at) {
  const offset = zone.offsetAt(at);
  const wall = new Date(at + offset).toISOString().slice(0, 16);
  const total = Math.abs(Math.round(offset / MINUTE_MS));
  return `${wall}${offset < 0 ? "-" : "+"}${two(Math.floor(total / 60))}:${two(total % 60)}`;
}

const WEEKDAY_NAMES = ["воскресенье", "понедельник", "вторник", "среда", "четверг", "пятница", "суббота"];
const WEEKDAY_ON = ["в воскресенье", "в понедельник", "во вторник", "в среду", "в четверг", "в пятницу", "в субботу"];

function joined(words) {
  return words.length > 1 ? `${words.slice(0, -1).join(", ")} и ${words[words.length - 1]}` : words[0];
}

// Неделя: одинаковые повторы — одной строкой («каждый день в 10:00 — Планёрка»), у остальных — день и время.
function weekSaid(events, zone, today) {
  const groups = new Map();
  for (const event of events) {
    const first = Math.max(event.allDay ? event.day : floorDay(toWall(zone, event.start)), today);
    const index = Math.round((first - today) / DAY_MS);
    const time = event.allDay ? "" : event.start < toUtc(zone, today) ? "" : clockOf(zone, event.start);
    const key = `${event.title}\u0000${time}`;
    const group = groups.get(key) ?? { title: event.title, time, days: [] };
    if (!group.days.includes(index)) group.days.push(index);
    groups.set(key, group);
  }
  const dayWord = (index) => (index === 0 ? "сегодня" : index === 1 ? "завтра" : WEEKDAY_ON[new Date(today + index * DAY_MS).getUTCDay()]);
  return [...groups.values()].map(({ title, time, days }) => {
    const weekdays = days.map((index) => new Date(today + index * DAY_MS).getUTCDay());
    const span = days[days.length - 1] - days[0] + 1;
    // Сегодняшнее могло уже пройти: «каждый день» и «по будням» считаются и с завтрашнего.
    const from = Math.min(days[0], 1);
    let when;
    if (days.length >= 3 && from === days[0] && span === days.length && days[days.length - 1] === 6) when = "каждый день";
    else if (days.length >= 3 && weekdays.every((day) => day >= 1 && day <= 5) && days.length === countWeekdays(today, from)) when = "по будням";
    else when = joined(days.map(dayWord));
    return time ? `${when} в ${time} — ${title}` : `${when} — ${title}`;
  });
}

// Сколько будних дней от дня index до конца недели окна.
function countWeekdays(today, index) {
  let count = 0;
  for (let one = index; one <= 6; one += 1) {
    const day = new Date(today + one * DAY_MS).getUTCDay();
    if (day >= 1 && day <= 5) count += 1;
  }
  return count;
}

// Фраза: «что у меня сегодня», «что у меня завтра», «на неделе».
export async function agendaSay({ request }) {
  const text = String(request?.text ?? "").toLowerCase();
  const tomorrow = text.includes("завтра");
  const week = text.includes("недел");
  let cache;
  try {
    cache = await source(TIMEOUT);
  } catch (error) {
    return message(error);
  }
  if (!cache) return "Календарь не подключён: добавьте ссылку iCal в настройках приложения «Календарь» в пульте.";
  const { zone } = listener(cache, request?.timezone);
  const now = Date.now();
  const today = todayOf(zone);
  const when = week ? "на неделе" : tomorrow ? "завтра" : "сегодня";
  if (week) {
    const events = occurrences(cache, zone, Math.max(now - HOUR_MS, toUtc(zone, today)), toUtc(zone, today + 7 * DAY_MS));
    if (events.length === 0) return `В календаре ${when} ничего нет.`;
    const said = weekSaid(events, zone, today);
    return `В календаре ${when}: ${said.slice(0, 6).join("; ")}${said.length > 6 ? ` и ещё ${said.length - 6}` : ""}.`;
  }
  const dayStart = toUtc(zone, today + (tomorrow ? DAY_MS : 0));
  const dayEnd = toUtc(zone, today + (tomorrow ? 2 : 1) * DAY_MS);
  const events = occurrences(cache, zone, tomorrow ? dayStart : Math.max(now - HOUR_MS, dayStart), dayEnd);
  if (events.length === 0) return `В календаре ${when} ничего нет.`;
  const said = events.map((one) => {
    if (one.allDay) return one.title;
    // Началось раньше этого дня — когда кончится или весь день.
    if (one.start < dayStart) return one.end <= dayEnd ? `до ${clockOf(zone, one.end)} — ${one.title}` : one.title;
    return `в ${clockOf(zone, one.start)} — ${one.title}`;
  });
  return `В календаре ${when}: ${said.slice(0, 6).join("; ")}${said.length > 6 ? ` и ещё ${said.length - 6}` : ""}.`;
}

// Инструмент calendar_events: события на весь день — датой, остальные — местным временем со сдвигом пояса.
export async function calendarEvents({ days, timezone }) {
  const span = Math.min(Math.max(Math.round(Number(days)) || 2, 1), 14);
  let cache;
  try {
    cache = await source(TIMEOUT);
  } catch (error) {
    return { configured: true, error: message(error) };
  }
  if (!cache) return { configured: false, note: "Календарь не подключён: нужна ссылка iCal в настройках приложения." };
  const { zone, name } = listener(cache, timezone);
  const today = todayOf(zone);
  const events = occurrences(cache, zone, toUtc(zone, today), toUtc(zone, today + span * DAY_MS));
  const shown = events.slice(0, 40).map((one) => {
    const base = one.place ? { title: one.title, place: one.place } : { title: one.title };
    if (one.allDay) {
      const last = one.days > 1 ? { lastDate: isoDate(one.day + (one.days - 1) * DAY_MS) } : {};
      return { ...base, allDay: true, date: isoDate(one.day), ...last, weekday: WEEKDAY_NAMES[new Date(one.day).getUTCDay()] };
    }
    const weekday = WEEKDAY_NAMES[new Date(toWall(zone, one.start)).getUTCDay()];
    return { ...base, allDay: false, start: isoLocal(zone, one.start), end: isoLocal(zone, one.end), weekday };
  });
  return {
    configured: true,
    timezone: name ?? "пояс шлюза",
    today: isoDate(today),
    events: shown,
    ...(events.length > shown.length ? { more: events.length - shown.length } : {}),
  };
}

// Виджет на столе пульта: сегодня и ближайшие дни. Время виджет показывает по поясу браузера, день события на весь
// день — по date.
export async function panelEvents() {
  let cache;
  try {
    cache = await source(TIMEOUT);
  } catch (error) {
    return { configured: true, error: message(error), events: [] };
  }
  if (!cache) return { configured: false, events: [] };
  const { zone } = listener(cache, null);
  const today = todayOf(zone);
  const events = occurrences(cache, zone, Math.max(Date.now() - HOUR_MS, toUtc(zone, today)), toUtc(zone, today + 7 * DAY_MS));
  return {
    configured: true,
    events: events.slice(0, 30).map((one) => ({
      title: one.title,
      place: one.place,
      start: one.start,
      end: one.end,
      allDay: one.allDay,
      ...(one.allDay ? { date: isoDate(one.day) } : {}),
    })),
  };
}
