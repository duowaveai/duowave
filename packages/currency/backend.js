// Курсы валют: официальные курсы ЦБ России (cbr-xml-daily.ru — зеркало XML ЦБ в JSON, без ключа). Голосом — «курс
// доллара», «Мозгу» — инструмент currency_rates, столу пульта — panelRates (docs/plans/desk.md). Курс держится час.

const DAILY = "https://www.cbr-xml-daily.ru/daily_json.js";
const TTL_MS = 60 * 60 * 1000;
// ЦБ не отвечает — ответ по прежнему курсу, если ему не больше суток.
const STALE_MS = 24 * 60 * 60 * 1000;
// Вызов голоса, «Мозга» и пульта ограничен 2 с.
const TIMEOUT = 1700;
const MAIN = ["USD", "EUR", "CNY"];

// Уточнение решает, какая это валюта: «канадского доллара» — CAD, «египетского фунта» — EGP, «фунта стерлингов» — GBP.
// Считается оно только рядом с названием валюты или словом «валюта»: «курс английского языка» — не фунт. Само по себе
// (alone) — только «США» и «стерлингов»: они и без названия о валюте. whole — только целым словом.
const QUALIFIERS = [
  ["американск", "USD"], ["сша", "USD", true, true], ["канадск", "CAD"], ["австралийск", "AUD"], ["новозеландск", "NZD"],
  ["гонконгск", "HKD"], ["сингапурск", "SGD"], ["египетск", "EGP"], ["британск", "GBP"], ["английск", "GBP"],
  ["стерлинг", "GBP", false, true], ["японск", "JPY"], ["китайск", "CNY"], ["швейцарск", "CHF"], ["турецк", "TRY"],
  ["казахстанск", "KZT"], ["казахск", "KZT"], ["белорусск", "BYN"], ["украинск", "UAH"], ["эмиратск", "AED"],
  ["индийск", "INR"], ["индонезийск", "IDR"], ["южнокорейск", "KRW"], ["корейск", "KRW"], ["польск", "PLN"],
  ["шведск", "SEK"], ["чешск", "CZK"], ["норвежск", "NOK"], ["датск", "DKK"], ["армянск", "AMD"], ["грузинск", "GEL"],
  ["азербайджанск", "AZN"], ["таджикск", "TJS"], ["киргизск", "KGS"], ["кыргызск", "KGS"], ["узбекск", "UZS"],
  ["молдавск", "MDL"], ["румынск", "RON"], ["венгерск", "HUF"], ["сербск", "RSD"], ["болгарск", "BGN"],
  ["бразильск", "BRL"], ["южноафриканск", "ZAR"], ["тайск", "THB"], ["таиландск", "THB"], ["вьетнамск", "VND"],
  ["катарск", "QAR"], ["туркменск", "TMT"], ["европейск", "EUR"],
];
// Как называют валюту голосом: основа слова → код, когда уточнения нет. «Евро» — только целым словом: «европу» — не
// евро. «Рубль» и «валюты» без названия — главные курсы.
const NAMES = [
  ["доллар", "USD"], ["бакс", "USD"], ["евро", "EUR", true], ["юан", "CNY"], ["фунт", "GBP"], ["иен", "JPY"],
  ["йен", "JPY"], ["франк", "CHF"], ["лир", "TRY"], ["тенге", "KZT", true], ["гривн", "UAH"], ["дирхам", "AED"],
  ["рупи", "INR"], ["вон", "KRW"], ["злот", "PLN"], ["крон", "SEK"], ["драм", "AMD"], ["лари", "GEL", true],
  ["манат", "AZN"], ["сомони", "TJS"], ["сом", "KGS"], ["сум", "UZS"], ["форинт", "HUF"], ["рубл", "MAIN"],
  ["валют", "MAIN"],
];
const SAID = {
  USD: "доллар", EUR: "евро", CNY: "юань", GBP: "фунт", CHF: "франк", BYN: "белорусский рубль", AED: "дирхам",
  CAD: "канадский доллар", AUD: "австралийский доллар", SGD: "сингапурский доллар", HKD: "гонконгский доллар",
};

function words(text) {
  return String(text ?? "")
    .toLowerCase()
    .replace(/ё/g, "е")
    .replace(/[^\p{L}\p{N}]+/gu, " ")
    .trim()
    .split(" ")
    .filter(Boolean);
}

// После основы — только окончание: «доллар|ами», «канадск|ого». Длиннее — другое слово: «сум|асшедшего» — не сум.
const ENDING = 3;

// Строка таблицы для слова или null.
function found(table, word) {
  if (!word) return null;
  return table.find(([stem, , whole]) => (whole ? word === stem : word.startsWith(stem) && word.length - stem.length <= ENDING)) ?? null;
}

// Код валюты в словах: первое её название — с уточнением прямо перед ним или после («канадского доллара», «доллара
// США»), — или код (USD); MAIN — валюта не названа, а спрошено о курсах вообще. Уточнение без названия рядом — не
// валюта: «курс китайского» — null.
function codeOf(text) {
  const list = words(text);
  let main = false;
  for (let index = 0; index < list.length; index += 1) {
    const named = found(NAMES, list[index]);
    if (!named) {
      const alone = found(QUALIFIERS, list[index]);
      if (alone?.[3]) return alone[1];
      continue;
    }
    const before = found(QUALIFIERS, list[index - 1]);
    if (before) return before[1];
    // После названия: «доллара США», «фунта стерлингов». В «доллара китайского юаня» уточнение — юаня.
    const after = found(QUALIFIERS, list[index + 1]);
    if (after && !found(NAMES, list[index + 2])) return after[1];
    if (named[1] === "MAIN") main = true;
    else return named[1];
  }
  const code = list.find((word) => /^[a-z]{3}$/.test(word));
  if (code) return code === "rub" ? "MAIN" : code.toUpperCase();
  return main ? "MAIN" : null;
}

// Курс ЦБ: из кэша, свежий или — если ЦБ не ответил — вчерашний.
async function daily(timeoutMs) {
  let cached = null;
  try {
    cached = host.kv.get("daily");
  } catch (error) {
    console.warn(`кэш курса не читается: ${error.message}`);
  }
  if (cached && Date.now() - cached.at < TTL_MS) return cached.data;
  let data;
  try {
    const answer = await host.http.fetch(DAILY, { timeoutMs });
    if (!answer.ok) throw new Error(`ответ ${answer.status}`);
    data = await answer.json();
    if (!data?.Valute) throw new Error("в ответе нет курсов");
  } catch (error) {
    if (cached && Date.now() - cached.at < STALE_MS) return cached.data;
    console.warn(`ЦБ: ${error.message}`);
    throw new Error("ЦБ сейчас не отвечает — курс не узнать. Попробуйте чуть позже.");
  }
  // В кэш — только нужное: значение kv не больше 64 КБ.
  const valute = {};
  for (const [code, one] of Object.entries(data.Valute)) {
    valute[code] = { Nominal: one.Nominal, Name: one.Name, Value: one.Value, Previous: one.Previous };
  }
  const compact = { Date: data.Date, Valute: valute };
  try {
    host.kv.set("daily", { at: Date.now(), data: compact });
  } catch (error) {
    console.warn(`кэш курса не записался: ${error.message}`);
  }
  return compact;
}

function rateOf(data, code) {
  const valute = data?.Valute?.[code];
  if (!valute) return null;
  const nominal = valute.Nominal || 1;
  const value = valute.Value / nominal;
  const previous = valute.Previous / nominal;
  return { code, name: valute.Name, value: Math.round(value * 100) / 100, change: Math.round((value - previous) * 100) / 100 };
}

function rubles(value) {
  const total = Math.round(value * 100);
  const whole = Math.floor(total / 100);
  const kopecks = total % 100;
  if (!whole) return `${kopecks} коп.`;
  return kopecks ? `${whole} руб. ${kopecks} коп.` : `${whole} руб.`;
}

// Как курс звучит. Мелкую валюту ЦБ даёт за 10, 100 или 10 000 единиц и называет во множественном числе — так и
// сказано: «100 японских иен — 51 руб.», «10 египетских фунтов — 16 руб. 50 коп.».
function spoken(data, code) {
  const valute = data?.Valute?.[code];
  if (!valute) return null;
  const nominal = valute.Nominal || 1;
  const change = Math.round((valute.Value - valute.Previous) * 100) / 100;
  const name = nominal > 1 ? `${nominal} ${String(valute.Name).toLowerCase()}` : SAID[code] ?? String(valute.Name).toLowerCase();
  const trend = change > 0 ? `, вырос на ${rubles(change)}` : change < 0 ? `, упал на ${rubles(-change)}` : "";
  return `${name} — ${rubles(valute.Value)}${trend}`;
}

// Фраза: «курс доллара», «курс канадского доллара», «сколько стоит евро», «курсы валют». Валюту фразы называют сами, и
// она читается из всех слов человека (request.text): слот {kind} — уточнение перед названием, {rest} — хвост («США»,
// «на сегодня»). currency — валюта словами, когда зовут без request.
export async function rateSay({ currency, request }) {
  const code = codeOf(request?.text) ?? codeOf(currency);
  // Валюта не нашлась — так и сказать, а не читать курс наугад.
  if (!code) return currency ? `Не знаю такую валюту: «${currency}».` : "Не поняла, курс какой валюты нужен.";
  const codes = code === "MAIN" ? MAIN : [code];
  let data;
  try {
    data = await daily(TIMEOUT);
  } catch (error) {
    return error.message;
  }
  const parts = codes.map((one) => spoken(data, one)).filter(Boolean).join("; ");
  // «…коп.» уже кончается точкой.
  return parts ? `Курс ЦБ на сегодня: ${parts}${parts.endsWith(".") ? "" : "."}` : "ЦБ не прислал курс этой валюты.";
}

// Инструмент currency_rates.
export async function currencyRates({ codes }) {
  let data;
  try {
    data = await daily(TIMEOUT);
  } catch (error) {
    return { error: error.message };
  }
  const wanted = Array.isArray(codes) && codes.length ? codes.map((code) => String(code).toUpperCase()) : MAIN;
  return { date: data.Date, rates: wanted.map((code) => rateOf(data, code)).filter(Boolean) };
}

// Виджет на столе пульта.
export async function panelRates() {
  const data = await daily(TIMEOUT);
  return { date: data.Date, rates: MAIN.map((code) => rateOf(data, code)).filter(Boolean) };
}

const money = (value) => Number(value).toFixed(2).replace(".", ",");

// Карточка на столе пульта (docs/plugins/cards.md): курс ЦБ и изменение к прошлому дню. Рисует пульт.
export async function ratesCard() {
  let data;
  try {
    data = await panelRates();
  } catch (error) {
    return { empty: String(error?.message ?? error) };
  }
  const day = String(data.date ?? "").slice(0, 10).split("-").reverse().slice(0, 2).join(".");
  return {
    rows: data.rates.map((rate) => ({
      text: `${rate.code} · ${rate.name}`,
      value: `${money(rate.value)} ₽`,
      delta: {
        text: rate.change === 0 ? "0" : `${rate.change > 0 ? "+" : "−"}${money(Math.abs(rate.change))}`,
        tone: rate.change > 0 ? "up" : rate.change < 0 ? "down" : "flat",
      },
    })),
    cap: day ? `ЦБ России на ${day}` : "ЦБ России",
    say: data.rates.map((rate) => `${rate.name}: ${money(rate.value)} руб.`).join("; "),
  };
}
