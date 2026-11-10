// Виджет «Календарь» на столе пульта (docs/plans/desk.md): ближайшие события — сегодня, завтра и дальше. Время — по
// поясу браузера, событие на весь день — по своей дате (date), без пояса.
import { h } from "preact";
import { useEffect, useState } from "preact/hooks";
import { definePlugin, icons, useApi, usePanelState } from "@duowave/panel-sdk";

const DAY_MS = 24 * 60 * 60 * 1000;

function midnight() {
  const today = new Date();
  today.setHours(0, 0, 0, 0);
  return today.getTime();
}

// Начало дня события по часам браузера: у события на весь день — его дата.
function dayStart(event) {
  if (event.allDay && event.date) {
    const [y, m, d] = event.date.split("-").map(Number);
    return new Date(y, m - 1, d).getTime();
  }
  const day = new Date(event.start);
  day.setHours(0, 0, 0, 0);
  return day.getTime();
}

function dayOf(event) {
  const diff = Math.round((dayStart(event) - midnight()) / DAY_MS);
  if (diff <= 0) return "сегодня";
  if (diff === 1) return "завтра";
  return new Date(dayStart(event)).toLocaleDateString("ru-RU", { weekday: "long", day: "numeric" });
}

const clock = (at) => new Date(at).toLocaleTimeString("ru-RU", { hour: "2-digit", minute: "2-digit" });

function timeOf(event) {
  if (event.allDay) return "весь день";
  // Началось раньше сегодняшнего дня — до скольких идёт сегодня.
  if (event.start < midnight()) return event.end <= midnight() + DAY_MS ? `до ${clock(event.end)}` : "весь день";
  return clock(event.start);
}

function Calendar() {
  const api = useApi("calendar");
  const { version, go } = usePanelState();
  const [data, setData] = useState(null);
  const [error, setError] = useState(null);
  useEffect(() => {
    let alive = true;
    api
      .call("panelEvents")
      .then((answer) => alive && (setData(answer), setError(null)))
      .catch((e) => alive && setError(e?.message ?? String(e)));
    return () => {
      alive = false;
    };
  }, [api, version]);
  const settings = h("button", { class: "w-btn", onClick: () => go("plugins", "installed", "calendar") }, "Настроить");
  if (error) return h("p", { class: "w-cap c-dot-offline m-0" }, error);
  if (!data) return h("p", { class: "w-cap m-0" }, "Читаю календарь…");
  if (!data.configured)
    return h(
      "div",
      { class: "flex flex-col gap-8" },
      h("p", { class: "w-cap m-0" }, "Добавьте ссылку iCal своего календаря в настройках плагина — Google, Яндекс и iCloud её дают."),
      settings,
    );
  // Ссылка не работает или календарь не ответил: шлюз говорит, что не так.
  if (data.error) return h("div", { class: "flex flex-col gap-8" }, h("p", { class: "w-cap c-dot-offline m-0" }, data.error), settings);
  if (data.events.length === 0) return h("p", { class: "w-cap m-0" }, "На неделе в календаре пусто.");
  let lastDay = null;
  const rows = [];
  for (const event of data.events.slice(0, 6)) {
    const day = dayOf(event);
    if (day !== lastDay) {
      rows.push(h("p", { key: `day-${event.start}`, class: "w-cap m-0 calendar-day" }, day));
      lastDay = day;
    }
    rows.push(
      h(
        "div",
        { key: `${event.start}-${event.title}`, class: "w-row" },
        h("span", { class: "w-cap calendar-time" }, timeOf(event)),
        h("span", { class: "w-text" }, event.title),
      ),
    );
  }
  return h("div", { class: "flex flex-col" }, rows);
}

export default definePlugin({
  id: "calendar",
  slots: {
    "desk.widgets": [
      { id: "agenda", title: "Календарь", icon: icons.sparkle, size: "small", source: "плагин", order: 65, permission: "station.view", Component: Calendar },
    ],
  },
});
