// Виджет «Радио» на столе пульта (docs/plans/desk.md; прежде — карточка главного экрана): что играет сейчас и
// сколько, свои станции, какие из них молчали на колонке. Preact и SDK — пульта: import map в его index.html ведёт
// сюда «preact» и «@duowave/panel-sdk». В шлюз — только через useApi, к функциям из contributes.panel.calls.
import { h } from "preact";
import { useEffect, useState } from "preact/hooks";
import { definePlugin, icons, useApi, usePanelState, useStation, Pill } from "@duowave/panel-sdk";

function minutes(since) {
  const total = Math.max(0, Math.round((Date.now() - since) / 60000));
  if (total < 1) return "только что";
  if (total < 60) return `${total} мин`;
  return `${Math.floor(total / 60)} ч ${total % 60} мин`;
}

function Radio() {
  const api = useApi("radio");
  const { version } = usePanelState();
  // Что играет — на колонке пульта: у каждой колонки своё.
  const { device } = useStation();
  const [data, setData] = useState(null);
  const [error, setError] = useState(null);
  const [, tick] = useState(0);
  useEffect(() => {
    let alive = true;
    api
      .call("panelStations", device ? { device } : {})
      .then((answer) => alive && (setData(answer), setError(null)))
      .catch((e) => alive && setError(e?.message ?? String(e)));
    return () => {
      alive = false;
    };
  }, [api, version, device]);
  // «Играет 44 минуты» идёт само: раз в минуту.
  useEffect(() => {
    const id = setInterval(() => tick((n) => n + 1), 60000);
    return () => clearInterval(id);
  }, []);
  if (error) return h("p", { class: "w-cap c-dot-offline m-0" }, error);
  if (!data) return h("p", { class: "w-cap m-0" }, "Читаю…");
  const broken = data.stations.filter((station) => station.broken);
  // Колонка пропала и не вернулась к until — у неё уже ничего не играет (проверяется с ежеминутным тиком).
  const now = data.now && !(data.now.until && Date.now() > data.now.until) ? data.now : null;
  return h(
    "div",
    { class: "flex flex-col gap-10" },
    now
      ? h(
          "div",
          { class: "flex flex-col" },
          h("span", { class: "w-cap" }, `играет ${minutes(now.since)}`),
          h("span", { class: "w-strong" }, now.title),
        )
      : h("p", { class: "w-cap m-0" }, data.last ? `Сейчас не играет. Последнее: ${data.last}.` : "Скажите: «Алиса, включи радио» — и название станции или жанр."),
    h(
      "div",
      { class: "flex flex-wrap gap-6" },
      data.stations.slice(0, 8).map((station) => h(Pill, { key: station.id, tone: station.broken ? "off" : "own" }, station.title)),
    ),
    broken.length > 0 && h("p", { class: "w-cap m-0" }, `Молчали на колонке: ${broken.map((station) => station.title).join(", ")} — идут последними.`),
  );
}

export default definePlugin({
  id: "radio",
  slots: {
    "desk.widgets": [
      { id: "stations", title: "Радио", icon: icons.music, size: "small", initial: true, source: "плагин", order: 60, permission: "station.view", Component: Radio },
    ],
  },
});
