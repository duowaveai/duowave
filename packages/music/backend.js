// Своя музыка: файлы из папки дома (CONTENT_DIR/music). Перенос источника шлюза (FolderLibrary в
// gateway/src/skills/media/sources.rs) в плагин, этап 10 плана: файлы ищет и читает шлюз (письмо
// system/content по шине), плагин выбирает, что играть; ссылку для колонки делает шлюз по `content`.

const LIMIT = 100;

// Источник для встроенного навыка музыки: по запросу — лучшее, без запроса — вперемешку.
export async function library({ query, limit = LIMIT }) {
  return await host.bus.call("system/content", "music", { query: query ?? "", limit: Math.min(Number(limit) || LIMIT, LIMIT) });
}

// «Включи мою музыку Битлз».
export async function playMine({ query }) {
  const items = await host.bus.call("system/content", "music", { query: query ?? "", limit: LIMIT });
  if (items.length === 0) {
    return query ? `В моей музыке нет «${query}».` : "В папке музыки пусто: положите файлы в content/music.";
  }
  return { say: query ? `Включаю: ${items[0].title}` : "Включаю вашу музыку.", play: items };
}
