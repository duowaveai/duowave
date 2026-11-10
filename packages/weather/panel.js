// Виджет «Погода» на столе пульта (docs/plans/desk.md): сейчас крупно и три дня вперёд. Preact и SDK — пульта: import
// map в его index.html ведёт сюда «preact» и «@duowave/panel-sdk». В шлюз — только через useApi, к функциям из
// contributes.panel.calls. Рамку виджета — название, значок, «…» — даёт стол.
import { h } from "preact";
import { useEffect, useState } from "preact/hooks";
import { definePlugin, icons, useApi, usePanelState } from "@duowave/panel-sdk";

const signed = (value) => `${value > 0 ? "+" : ""}${value}°`;

function Weather() {
  const api = useApi("weather");
  const { version } = usePanelState();
  const [data, setData] = useState(null);
  const [error, setError] = useState(null);
  useEffect(() => {
    let alive = true;
    api
      .call("panelWeather")
      .then((answer) => alive && (setData(answer), setError(null)))
      .catch((e) => alive && setError(e?.message ?? String(e)));
    return () => {
      alive = false;
    };
  }, [api, version]);
  if (error) return h("p", { class: "w-cap m-0" }, error);
  if (!data) return h("p", { class: "w-cap m-0" }, "Узнаю погоду…");
  if (data.error) return h("p", { class: "w-cap m-0" }, data.error);
  const now = data.now;
  const line = [now.text, now.wind !== null ? `ветер ${now.wind} м/с` : null].filter(Boolean).join(" · ");
  return h(
    "div",
    { class: "flex flex-col gap-10" },
    h(
      "div",
      { class: "flex items-baseline gap-10" },
      h("span", { class: "w-big" }, signed(now.temperature)),
      h("span", { class: "w-cap" }, line),
    ),
    data.days.length > 0 &&
      h(
        "div",
        { class: "weather-days" },
        data.days.map((day) =>
          h(
            "div",
            { key: day.date, class: "w-day", title: `${day.text}${day.rain ? `, осадки ${day.rain}%` : ""}` },
            h("p", { class: "w-cap m-0" }, day.label),
            h("p", { class: "m-0 weather-day-temp" }, signed(day.max)),
          ),
        ),
      ),
    h("p", { class: "w-cap m-0" }, data.place),
  );
}

export default definePlugin({
  id: "weather",
  slots: {
    "desk.widgets": [
      { id: "now", title: "Погода", icon: icons.weather, size: "small", initial: true, source: "плагин", order: 5, permission: "station.view", Component: Weather },
    ],
  },
});
