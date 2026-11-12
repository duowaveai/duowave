// Виджет «Список покупок» на столе пульта (docs/plans/desk.md): что купить; нажатие вычёркивает, поле внизу —
// добавить. Тот же список, что колонка ведёт голосом.
import { h } from "preact";
import { useEffect, useState } from "preact/hooks";
import { definePlugin, icons, useApi, usePanelState } from "@duowave/panel-sdk";

function Shopping() {
  const api = useApi("shopping");
  const { version } = usePanelState();
  const [list, setList] = useState(null);
  const [text, setText] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState(null);
  // true — сделано; false — не вышло, а почему — в error (шлюз не ответил или список полон).
  const run = (fn, input) => {
    setBusy(true);
    setError(null);
    return api
      .call(fn, input)
      .then((answer) => {
        setList(answer.items);
        if (answer.error) setError(answer.error);
        return !answer.error;
      })
      .catch((e) => {
        setError(e?.message ?? String(e));
        return false;
      })
      .finally(() => setBusy(false));
  };
  useEffect(() => {
    void run("panelList");
  }, [api, version]);
  if (!list) return h("p", { class: "w-cap m-0" }, error ?? "Читаю…");
  const submit = (event) => {
    event.preventDefault();
    if (!text.trim()) return;
    void run("panelAdd", { text }).then((done) => done && setText(""));
  };
  return h(
    "div",
    { class: "flex flex-col gap-8" },
    list.length === 0
      ? h("p", { class: "w-cap m-0" }, "Пусто. Скажите: «добавь молоко в список покупок».")
      : h(
          "div",
          { class: "w-rows" },
          list.map((entry) =>
            h(
              "button",
              { key: entry.id, class: "w-row w-row-btn shopping-item", disabled: busy, title: "Вычеркнуть", onClick: () => run("panelRemove", { id: entry.id }) },
              h("span", { class: "shopping-box", "aria-hidden": "true" }),
              h("span", { class: "w-text" }, entry.text),
            ),
          ),
        ),
    h(
      "form",
      { class: "shopping-add", onSubmit: submit },
      h("input", {
        class: "field",
        value: text,
        placeholder: "Добавить…",
        "aria-label": "Добавить в список покупок",
        maxLength: 60,
        onInput: (event) => setText(event.currentTarget.value),
      }),
      h("button", { class: "w-btn", type: "submit", disabled: busy || !text.trim() }, "Добавить"),
    ),
    error && h("p", { class: "w-cap c-dot-offline m-0" }, error),
  );
}

export default definePlugin({
  id: "shopping",
  slots: {
    "desk.widgets": [
      { id: "list", title: "Список покупок", icon: icons.list, size: "small", initial: true, source: "приложение", order: 50, permission: "station.view", Component: Shopping },
    ],
  },
});
