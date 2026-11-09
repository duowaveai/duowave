// Погода: Open-Meteo (без ключа) — геокодер для города и прогноз. Перенос встроенного навыка шлюза
// (gateway/src/skills/lookups/weather_skill.rs) в плагин, этап 10 плана: те же ответы, то же место по умолчанию.
// Встроенный навык остаётся запасным: плагин вернул null или упал — отвечает он.

const GEOCODE = "https://geocoding-api.open-meteo.com/v1/search";
const FORECAST = "https://api.open-meteo.com/v1/forecast";
// Голосовой вызов ограничен 2 с: город — из кэша, прогноз — одним запросом.
const TIMEOUT = 1700;

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

// «… в москве» в конце вопроса — место.
function placeIn(text) {
  const found = normalize(text).match(/(?:^|\s)(?:в|во)\s+([\p{L}-]+(?:\s[\p{L}-]+)?)$/u);
  return found ? found[1] : null;
}

async function usualPlace(request) {
  const own = String(host.settings.get("place") ?? "").trim();
  if (own) return own;
  const panel = await host.home.weatherPlace().catch(() => null);
  if (panel) return panel;
  const zone = String(request?.timezone ?? "").split("/").pop();
  return zone ? zone.replace(/_/g, " ") : null;
}

async function locate(place) {
  const coordinates = place.match(/^\s*(-?\d+(?:\.\d+)?)\s*,\s*(-?\d+(?:\.\d+)?)\s*$/);
  if (coordinates) return { name: "Здесь", latitude: Number(coordinates[1]), longitude: Number(coordinates[2]) };
  const key = `place:${normalize(place)}`;
  const known = host.kv.get(key);
  if (known) return known;
  // «в Москве» → Москва: окончания падежа пробуются по очереди, как у встроенного навыка.
  const variants = [...new Set([place, place.replace(/е$/, "а"), place.replace(/е$/, ""), place.replace(/и$/, "ь")])];
  for (const name of variants) {
    const answer = await host.http.fetch(`${GEOCODE}?name=${encodeURIComponent(name)}&count=1&language=ru&format=json`, { timeoutMs: TIMEOUT });
    const first = (await answer.json())?.results?.[0];
    if (first) {
      const found = { name: first.name, latitude: first.latitude, longitude: first.longitude };
      host.kv.set(key, found);
      return found;
    }
  }
  return null;
}

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

async function lookup(placeName, days) {
  const place = await locate(placeName);
  if (!place) return { text: `Не нашла город «${placeName}».` };
  const url = `${FORECAST}?latitude=${place.latitude}&longitude=${place.longitude}` +
    "&current=temperature_2m,apparent_temperature,weather_code,wind_speed_10m" +
    "&daily=weather_code,temperature_2m_max,temperature_2m_min,precipitation_probability_max" +
    "&timezone=auto&forecast_days=7&wind_speed_unit=ms";
  const forecast = await (await host.http.fetch(url, { timeoutMs: TIMEOUT })).json();
  return { text: describe(forecast, place.name, days), now: days === 0 ? forecast.current?.temperature_2m : undefined };
}

function daysOf(when) {
  const text = normalize(when);
  if (text.includes("недел") || text === "week") return 7;
  if (text.includes("завтра") || text === "tomorrow") return 1;
  return 0;
}

// Фраза: «какая погода завтра в москве».
export async function forecast({ when, request }) {
  const place = placeIn(when ?? "") ?? (await usualPlace(request));
  if (!place) return "Не знаю, для какого города узнать погоду.";
  const answer = await lookup(place, daysOf(when ?? ""));
  return answer.now === undefined ? answer.text : { say: answer.text, display: { temperature: answer.now } };
}

// Инструмент weather_now — «Мозгу» и утреннему шоу.
export async function weatherNow({ place, when, timezone }) {
  const name = String(place ?? "").trim() || (await usualPlace({ timezone }));
  if (!name) return "Место для погоды не задано.";
  return (await lookup(name, daysOf(when ?? "now"))).text;
}
