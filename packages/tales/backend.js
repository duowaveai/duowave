// Сказки: файлы из папки дома (CONTENT_DIR/tales) — источник для плеера шлюза, и сказка, которую сочиняет модель
// шлюза (ответ `model`, право llm.chat). Перенос встроенного навыка сказок в плагин, этап 10 плана.

export async function shelf({ query, limit = 50 }) {
  return await host.content.search("tales", query ?? "", Math.min(Number(limit) || 50, 100));
}

// «Включи сказку про колобка»: готовая из папки, а нет её — сочинённая.
export async function playTale({ query }) {
  const topic = String(query ?? "").replace(/^(про|о|об)\s+/, "");
  const items = await host.content.search("tales", topic, 50);
  if (items.length > 0) return { say: `Включаю сказку «${items[0].title}».`, play: items };
  return composeTale({ topic });
}

// «Сочини сказку про ёжика»: говорит модель шлюза.
export function composeTale({ topic }) {
  const about = String(topic ?? "").replace(/^(про|о|об)\s+/, "").trim();
  return {
    model: {
      system:
        "Ты рассказываешь детскую сказку вслух на умной колонке: всё, что ты пишешь, будет озвучено. Пиши простыми короткими " +
        "предложениями, без списков и разметки, с добрым концом. Сказка — на две-три минуты чтения.",
      prompt: about ? `Расскажи сказку про ${about}.` : "Расскажи новую сказку.",
      key: "tale",
      fresh: true,
    },
  };
}
