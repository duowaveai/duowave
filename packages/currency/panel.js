// Виджет «Курсы валют» на столе пульта (docs/plans/desk.md): доллар, евро и юань ЦБ на сегодня и как изменились.
import { h } from "preact";
import { useEffect, useState } from "preact/hooks";
import { definePlugin, icons, useApi, usePanelState } from "@duowave/panel-sdk";

const money = (value) => value.toLocaleString("ru-RU", { minimumFractionDigits: 2, maximumFractionDigits: 2 });

function Rates() {
  const api = useApi("currency");
  const { version } = usePanelState();
  const [data, setData] = useState(null);
  const [error, setError] = useState(null);
  useEffect(() => {
    let alive = true;
    api
      .call("panelRates")
      .then((answer) => alive && (setData(answer), setError(null)))
      .catch((e) => alive && setError(e?.message ?? String(e)));
    return () => {
      alive = false;
    };
  }, [api, version]);
  if (error) return h("p", { class: "w-cap c-dot-offline m-0" }, error);
  if (!data) return h("p", { class: "w-cap m-0" }, "Узнаю курсы…");
  return h(
    "div",
    { class: "flex flex-col gap-6" },
    h(
      "div",
      { class: "w-rows" },
      data.rates.map((rate) =>
        h(
          "div",
          { key: rate.code, class: "w-row" },
          h("span", { class: "w-text" }, rate.code, h("span", { class: "w-cap" }, ` · ${rate.name}`)),
          h("span", { class: "w-strong" }, `${money(rate.value)} ₽`),
          h(
            "span",
            { class: `w-cap currency-change ${rate.change > 0 ? "up" : rate.change < 0 ? "down" : ""}` },
            rate.change === 0 ? "0" : `${rate.change > 0 ? "+" : "−"}${money(Math.abs(rate.change))}`,
          ),
        ),
      ),
    ),
    data.date && h("p", { class: "w-cap m-0" }, `ЦБ на ${new Date(data.date).toLocaleDateString("ru-RU")}`),
  );
}

export default definePlugin({
  id: "currency",
  slots: {
    "desk.widgets": [
      { id: "rates", title: "Курсы валют", icon: icons.coins, size: "small", source: "плагин", order: 70, permission: "station.view", Component: Rates },
    ],
  },
});
