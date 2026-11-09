// Карточка «Радио» на главном экране пульта (docs/plans/plugins.md, этап 5): свои станции, какие из них молчали на
// колонке, что включали последним. Preact и SDK — пульта: import map в его index.html ведёт сюда «preact» и
// «@duowave/panel-sdk». В шлюз — только через useApi, к функциям из contributes.panel.calls.
import { h } from "preact";
import { useEffect, useState } from "preact/hooks";
import { definePlugin, useApi, usePanelState, Card, Note, Pill } from "@duowave/panel-sdk";

function Stations() {
  const api = useApi("radio");
  const { version } = usePanelState();
  const [data, setData] = useState(null);
  const [error, setError] = useState(null);
  useEffect(() => {
    api
      .call("panelStations")
      .then((answer) => {
        setData(answer);
        setError(null);
      })
      .catch((e) => setError(e?.message ?? String(e)));
  }, [api, version]);
  if (error) return h(Card, { title: "Радио" }, h(Note, { tone: "error" }, error));
  if (!data) return null;
  const broken = data.stations.filter((station) => station.broken);
  return h(
    Card,
    { title: "Радио", meta: data.last ? `последнее: ${data.last}` : undefined },
    h(Note, null, "Скажите: «Алиса, включи радио» — и название станции или жанр."),
    h(
      "div",
      { class: "flex flex-wrap gap-6" },
      data.stations.map((station) => h(Pill, { key: station.id, tone: station.broken ? "off" : "own" }, station.title)),
    ),
    broken.length > 0 &&
      h(Note, { tone: "warning" }, `Колонка молчала на: ${broken.map((station) => station.title).join(", ")}. Эти станции идут последними.`),
  );
}

export default definePlugin({
  id: "radio",
  slots: { "home.cards": [{ id: "stations", Component: Stations, order: 80, permission: "station.view" }] },
});
