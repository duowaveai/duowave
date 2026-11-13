// Новости: заголовки из RSS и Atom-лент. Перенос встроенного навыка шлюза (gateway/src/skills/lookups/news_skill.rs) в
// плагин, этап 10 плана. Ленты — выбранные в пульте («Дом» → «Коннекторы» → «Новости», письмо system/home), а без
// выбора — каталог ниже; ходит только к хостам из своих прав net:. Встроенный навык остаётся запасным.

const TOPICS = [{"id": "main", "title": "Главное", "words": ["главное", "главные", "последние", "свежие", "россия", "страна"]}, {"id": "world", "title": "В мире", "words": ["мир", "мире", "мировые", "международные", "за рубежом", "зарубежные"]}, {"id": "economy", "title": "Экономика", "words": ["экономика", "экономические", "бизнес", "финансы", "деньги", "рынки"]}, {"id": "tech", "title": "Технологии", "words": ["технологии", "техника", "гаджеты", "айти", "it", "интернет", "софт"]}, {"id": "science", "title": "Наука", "words": ["наука", "научные", "учёные", "космос", "исследования"]}, {"id": "sport", "title": "Спорт", "words": ["спорт", "спортивные", "футбол", "хоккей", "теннис", "матчи"]}, {"id": "culture", "title": "Культура", "words": ["культура", "кино", "фильмы", "книги", "фантастика", "сериалы"]}, {"id": "games", "title": "Игры", "words": ["игры", "игровые", "видеоигры", "геймеры"]}];
const FEEDS = [{"url": "https://tass.ru/rss/v2.xml","title": "ТАСС","topic": "main"},{"url": "https://www.interfax.ru/rss.asp","title": "Интерфакс","topic": "main"},{"url": "https://ria.ru/export/rss2/archive/index.xml","title": "РИА Новости","topic": "main"},{"url": "https://rg.ru/xml/index.xml","title": "Российская газета","topic": "main"},{"url": "https://lenta.ru/rss","title": "Lenta.ru","topic": "main"},{"url": "https://www.kommersant.ru/RSS/main.xml","title": "Коммерсантъ: главное","topic": "main"},{"url": "https://www.gazeta.ru/export/rss/first.xml","title": "Газета.Ru: первая полоса","topic": "main"},{"url": "https://www.mk.ru/rss/index.xml","title": "Московский комсомолец","topic": "main"},{"url": "https://vz.ru/rss.xml","title": "Взгляд","topic": "main"},{"url": "https://feeds.bbci.co.uk/russian/rss.xml","title": "Би-би-си: Русская служба","topic": "world"},{"url": "https://rss.dw.com/rdf/rss-ru-all","title": "DW на русском","topic": "world"},{"url": "https://lenta.ru/rss/news/world","title": "Lenta.ru: мир","topic": "world"},{"url": "https://www.kommersant.ru/RSS/section-world.xml","title": "Коммерсантъ: мир","topic": "world"},{"url": "https://rssexport.rbc.ru/rbcnews/news/30/full.rss","title": "РБК","topic": "economy"},{"url": "https://www.vedomosti.ru/rss/news","title": "Ведомости","topic": "economy"},{"url": "https://lenta.ru/rss/news/economics","title": "Lenta.ru: экономика","topic": "economy"},{"url": "https://www.kommersant.ru/RSS/section-economics.xml","title": "Коммерсантъ: экономика","topic": "economy"},{"url": "https://habr.com/ru/rss/news/?fl=ru","title": "Хабр: новости","topic": "tech"},{"url": "https://3dnews.ru/news/rss/","title": "3DNews","topic": "tech"},{"url": "https://www.ixbt.com/export/news.rss","title": "iXBT.com","topic": "tech"},{"url": "https://www.cnews.ru/inc/rss/news.xml","title": "CNews","topic": "tech"},{"url": "https://hightech.plus/feed.rss","title": "Хайтек+","topic": "tech"},{"url": "https://nplus1.ru/rss","title": "N + 1","topic": "science"},{"url": "https://naked-science.ru/feed","title": "Naked Science","topic": "science"},{"url": "https://indicator.ru/exports/rss","title": "Индикатор","topic": "science"},{"url": "https://elementy.ru/rss/news","title": "Элементы","topic": "science"},{"url": "https://lenta.ru/rss/news/science","title": "Lenta.ru: наука и техника","topic": "science"},{"url": "https://www.sport-express.ru/services/materials/news/se/","title": "Спорт-Экспресс","topic": "sport"},{"url": "https://www.championat.com/rss/news/","title": "Чемпионат","topic": "sport"},{"url": "https://lenta.ru/rss/news/sport","title": "Lenta.ru: спорт","topic": "sport"},{"url": "https://lenta.ru/rss/news/culture","title": "Lenta.ru: культура","topic": "culture"},{"url": "https://www.kommersant.ru/RSS/section-culture.xml","title": "Коммерсантъ: культура","topic": "culture"},{"url": "https://www.mirf.ru/feed","title": "Мир фантастики","topic": "culture"},{"url": "https://rss.stopgame.ru/rss_news.xml","title": "StopGame","topic": "games"},{"url": "https://www.igromania.ru/rss/news.rss","title": "Игромания","topic": "games"},{"url": "https://dtf.ru/rss/all","title": "DTF","topic": "games"}];
// Голосовой вызов ограничен 2 с: одна лента, быстрая.
const TIMEOUT = 1500;
const SHOWN = 5;

function normalize(text) {
  return String(text ?? "").toLowerCase().replace(/ё/g, "е").replace(/[^\p{L}\p{N}\s]+/gu, " ").replace(/\s+/g, " ").trim();
}

function topicOf(text) {
  const words = normalize(text).split(" ").filter((word) => word.length > 1 && !["про", "о", "об", "по", "в", "на"].includes(word));
  if (words.length === 0) return null;
  return TOPICS.find((topic) => topic.words.some((word) => words.some((said) => said.startsWith(word.slice(0, 5)) || word.startsWith(said.slice(0, 5))))) ?? null;
}

const ENTITIES = { amp: "&", lt: "<", gt: ">", quot: '"', apos: "'", nbsp: " ", laquo: "«", raquo: "»", mdash: "—", ndash: "–" };

function clean(text) {
  return String(text)
    .replace(/<!\[CDATA\[([\s\S]*?)\]\]>/g, "$1")
    .replace(/<[^>]+>/g, "")
    .replace(/&#(\d+);/g, (_, code) => String.fromCharCode(Number(code)))
    .replace(/&(\w+);/g, (whole, name) => ENTITIES[name] ?? whole)
    .replace(/\s+/g, " ")
    .trim();
}

// Заголовки ленты: <item><title> у RSS, <entry><title> у Atom.
function titles(xml) {
  const found = [];
  for (const block of xml.match(/<(item|entry)[\s>][\s\S]*?<\/(item|entry)>/g) ?? []) {
    const title = block.match(/<title[^>]*>([\s\S]*?)<\/title>/);
    if (title) {
      const text = clean(title[1]);
      if (text && !found.includes(text)) found.push(text);
    }
  }
  return found;
}

async function feedsFor(topic) {
  const chosen = await host.bus.call("system/home", "news-feeds").catch(() => []);
  const own = (chosen ?? []).filter((feed) => /^https:\/\//.test(feed.url ?? ""));
  const pool = own.length > 0 ? own : FEEDS;
  const wanted = topic ? pool.filter((feed) => feed.topic === topic.id) : pool.filter((feed) => !feed.topic || feed.topic === "main");
  return wanted.length > 0 ? wanted : pool;
}

async function read(topic, limit) {
  const feeds = await feedsFor(topic);
  // Лента наугад из подходящих: каждый раз немного другие заголовки, а запрос один.
  const start = Math.floor(Math.random() * feeds.length);
  for (let tried = 0; tried < Math.min(2, feeds.length); tried += 1) {
    const feed = feeds[(start + tried) % feeds.length];
    try {
      const answer = await host.http.fetch(feed.url, { timeoutMs: TIMEOUT, headers: { "User-Agent": "duowave-news/1.0" } });
      if (!answer.ok) continue;
      const found = titles(await answer.text()).slice(0, limit);
      if (found.length > 0) return { feed, titles: found };
    } catch (error) {
      console.warn(`${feed.url}: ${error.message}`);
    }
  }
  return null;
}

function spoken(result, topic) {
  const about = topic ? `${topic.title}` : "Главное";
  return `${about}: ${result.titles.map((title, index) => `${index + 1}) ${title}`).join(" ")}`;
}

// Фраза: «какие новости про науку».
export async function headlines({ topic: asked }) {
  const topic = topicOf(asked ?? "");
  const result = await read(topic, SHOWN);
  if (!result) return null;
  return spoken(result, topic);
}

// Инструмент news_headlines — «Мозгу» и утреннему шоу.
export async function newsHeadlines({ topic: asked, limit = SHOWN }) {
  const topic = topicOf(asked ?? "");
  const result = await read(topic, Math.min(Math.max(Number(limit) || SHOWN, 1), 10));
  return result ? spoken(result, topic) : "Новости сейчас не получить.";
}
