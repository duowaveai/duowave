// Утреннее шоу (этап 10 плана): собирает погоду и новости из других приложений (их инструменты — письма шины) и
// музыку — свою из папки (system/content) или радио плагина «Радио» (его инструмент radio_play отдаёт, что играть).
// Части идут одновременно: голосовой вызов ограничен 2 с, и не ответившая часть просто пропускается.

function greeting(timezone) {
  let hour = new Date().getUTCHours() + 3;
  try {
    hour = Number(new Intl.DateTimeFormat("ru-RU", { hour: "numeric", hour12: false, timeZone: timezone || "Europe/Moscow" }).format(new Date()));
  } catch {
    // без часового пояса — по Москве
  }
  if (hour < 5) return "Доброй ночи";
  if (hour < 12) return "Доброе утро";
  if (hour < 18) return "Добрый день";
  return "Добрый вечер";
}

// Ответ инструмента без хвоста для модели: первые фразы.
function short(text, sentences) {
  return String(text ?? "").split(/(?<=[.!?])\s+/).slice(0, sentences).join(" ").trim();
}

export async function show({ request }) {
  const wantNews = host.settings.get("news") !== false;
  const music = host.settings.get("music") ?? "своя музыка";
  const [weather, news, tracks] = await Promise.all([
    host.bus.call("app/weather", "weather_now", { timezone: request?.timezone }).catch(() => null),
    wantNews ? host.bus.call("app/news", "news_headlines", { limit: 2 }).catch(() => null) : null,
    music === "своя музыка" ? host.bus.call("system/content", "music", { query: "", limit: 30 }).catch(() => []) : [],
  ]);
  const parts = [`${greeting(request?.timezone)}!`];
  if (weather && !/^error|не задано|не получить/i.test(weather)) parts.push(`Погода. ${short(weather, 2)}`);
  if (news && !/^error|не получить/i.test(news)) parts.push(`Новости. ${short(news, 2)}`);
  if (tracks.length > 0) {
    parts.push("А теперь — музыка.");
    return { say: parts.join(" "), play: tracks };
  }
  // Без своей музыки — радио: radio_play отвечает {say, play}, и шоу включает его станции.
  const radio = await host.bus.call("app/radio", "radio_play", { station: "хиты" }).catch(() => null);
  if (Array.isArray(radio?.play) && radio.play.length > 0) {
    parts.push(`А теперь — ${radio.play[0].title}.`);
    return { say: parts.join(" "), play: radio.play };
  }
  parts.push("Хорошего дня!");
  return parts.join(" ");
}
