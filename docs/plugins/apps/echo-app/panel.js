// Эхо-приложение: «Долгий счёт» — пример долгой задачи (docs/plugins/AUTHORING.md, «Долгие задачи»): виджет стола
// и свой раздел с тем же содержимым (`#/echo-count` — открыть можно и без колонки, когда стола нет).
// Кнопка зовёт функцию программы `count` — та начинает задачу в шлюзе и сразу отвечает; дальше окно ничего не
// спрашивает: ход и конец задачи приходят сами через useTask(). «Отменить» просит программу остановиться.
// Пакет для проверки собирает scripts/dev/echo-app-package.sh.
import { h } from "preact";
import { useState } from "preact/hooks";
import { Action, Card, Note, definePlugin, icons, useApi, useTask } from "@duowave/panel-sdk";

const ID = "echo-app";
const ENDED = { done: "Готово", failed: "Не получилось", cancelled: "Отменена", interrupted: "Прервалась" };

// Линия хода: цвета — только токены темы.
function Line({ percent }) {
  const track = { height: "6px", borderRadius: "3px", background: "color-mix(in srgb, currentColor 14%, transparent)", overflow: "hidden" };
  const fill = { height: "100%", width: `${percent}%`, background: "var(--accent)", transition: "width 0.4s linear" };
  return h("div", { style: track, role: "progressbar", "aria-valuenow": percent, "aria-valuemin": 0, "aria-valuemax": 100 }, h("div", { style: fill }));
}

function Count() {
  const api = useApi(ID);
  const { task, ready, cancel } = useTask();
  const [busy, setBusy] = useState(false);
  const [refused, setRefused] = useState(null);

  const start = (input) => {
    setBusy(true);
    setRefused(null);
    api
      .call("count", input)
      // Шлюз отказал (задача уже идёт) — программа передаёт его слова как есть.
      .then((answer) => setRefused(answer?.refused?.error ?? null))
      .catch((error) => setRefused(error?.message ?? String(error)))
      .finally(() => setBusy(false));
  };

  if (!ready) return h(Note, null, "…");
  if (task?.state === "running") {
    const percent = task.percent ?? 0;
    return h(
      "div",
      { class: "w-rows" },
      h("div", { class: "w-big" }, `${percent}%`),
      h("div", { class: "w-cap" }, `${task.title} — ${task.text}`),
      h(Line, { percent }),
      h(Action, { label: task.cancelRequested ? "Отменяю…" : "Отменить", disabled: task.cancelRequested, onClick: () => cancel().catch(() => {}) }),
    );
  }
  const last = task
    ? `${ENDED[task.state] ?? task.state}: ${task.state === "done" ? `досчитал до ${task.result?.counted ?? "?"}` : (task.error ?? task.title)}`
    : "Ещё не считал.";
  return h(
    "div",
    { class: "w-rows" },
    h("div", { class: "w-cap" }, last),
    h(Action, { label: "Считать 20 секунд", busy, onClick: () => start({ seconds: 20 }) }),
    h(Action, { label: "Считать 2 минуты", disabled: busy, onClick: () => start({ seconds: 120 }) }),
    h(Action, { label: "Сбиться на половине", disabled: busy, onClick: () => start({ seconds: 10, fail: true }) }),
    refused && h(Note, { tone: "warning" }, refused),
  );
}

// Раздел: то же, что в виджете, и журнал последних задач под ним.
function Screen() {
  const { tasks } = useTask();
  return h(
    Card,
    { title: "Долгий счёт", meta: "пример долгой задачи" },
    h(Count),
    tasks.length > 0 &&
      h(
        "div",
        { class: "w-rows" },
        tasks.map((task) =>
          h("div", { class: "w-row", key: task.id }, `${task.title} — ${task.state === "running" ? `${task.percent ?? 0}%` : (ENDED[task.state] ?? task.state)}`),
        ),
      ),
  );
}

export default definePlugin({
  id: ID,
  sections: [{ id: "echo-count", title: "Долгий счёт", icon: icons.timer, nav: "more", Screen }],
  slots: {
    "desk.widgets": [{ id: "count", title: "Долгий счёт", icon: icons.timer, size: "small", initial: true, source: "приложение", Component: Count }],
  },
});
