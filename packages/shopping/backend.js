// Список покупок: свой, у шлюза (kv), без Яндекса. Голосом — добавить, прочитать, вычеркнуть, очистить; «Мозгу» —
// инструмент shopping_list; столу пульта — виджет со списком (panel.js, docs/plans/desk.md).

const MAX_ITEMS = 100;

function items() {
  return host.kv.get("items") ?? [];
}

function save(list) {
  host.kv.set("items", list.slice(0, MAX_ITEMS));
}

// Покупка — не длиннее 60 символов. Режется по символам, а не по половинкам UTF-16: эмодзи на границе не оставляет
// одинокую половинку, а с ней kv.set отказывает («значение не JSON»).
function clean(text) {
  const one = String(text ?? "").trim().replace(/\s+/g, " ");
  return one.length <= 60 ? one : Array.from(one.slice(0, 120)).slice(0, 60).join("").trim();
}

const same = (a, b) => a.toLowerCase().replace(/ё/g, "е") === b.toLowerCase().replace(/ё/g, "е");

// «Молоко, хлеб и яблоки» — три покупки.
function split(text) {
  return clean(text)
    .split(/\s*(?:,|\sи\s)\s*/u)
    .map(clean)
    .filter(Boolean);
}

// Что добавилось; full — не всё поместилось: в списке уже MAX_ITEMS покупок.
function add(texts) {
  const list = items();
  const added = [];
  let full = false;
  for (const text of texts) {
    if (list.some((entry) => same(entry.text, text))) continue;
    if (list.length >= MAX_ITEMS) {
      full = true;
      break;
    }
    const entry = { id: `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`, text, at: Date.now() };
    list.push(entry);
    added.push(entry);
  }
  if (added.length) save(list);
  return { added, full };
}

const FULL = `Список полон: ${MAX_ITEMS} покупок. Вычеркните купленное.`;

function remove(texts) {
  const list = items();
  const left = list.filter((entry) => !texts.some((text) => same(entry.text, text) || entry.id === text));
  save(left);
  return list.length - left.length;
}

// Речь — до 1000 знаков: длинный список — первые 20 покупок (не больше 700 знаков) и сколько ещё.
const SAID = 20;
const SAID_CHARS = 700;

function say(list) {
  if (list.length === 0) return "Список покупок пуст.";
  const words = [];
  let length = 0;
  for (const entry of list) {
    if (words.length >= SAID || length + entry.text.length > SAID_CHARS) break;
    words.push(entry.text);
    length += entry.text.length + 2;
  }
  const rest = list.length - words.length;
  if (rest > 0) return `В списке покупок, первые ${words.length}: ${words.join(", ")} и ещё ${rest}.`;
  return `В списке покупок: ${words.length > 1 ? `${words.slice(0, -1).join(", ")} и ${words[words.length - 1]}` : words[0]}.`;
}

export function addSay({ item }) {
  const texts = split(item);
  if (texts.length === 0) return "Что добавить в список покупок?";
  const { added, full } = add(texts);
  if (added.length === 0) return full ? FULL : "Это уже в списке.";
  const said = `Добавила в список: ${added.map((entry) => entry.text).join(", ")}.`;
  return full ? `${said} Остальное не поместилось: в списке ${MAX_ITEMS} покупок.` : said;
}

export function listSay() {
  return say(items());
}

export function removeSay({ item }) {
  const removed = remove(split(item));
  return removed ? "Вычеркнула." : "Такого в списке нет.";
}

export function clearSay() {
  save([]);
  return "Очистила список покупок.";
}

export function shoppingTool({ action, items: texts }) {
  const list = Array.isArray(texts) ? texts.map(clean).filter(Boolean) : [];
  if (action === "add") {
    const { added, full } = add(list);
    const answer = { added: added.map((entry) => entry.text), list: items().map((entry) => entry.text) };
    return full ? { ...answer, full: true, note: FULL } : answer;
  }
  if (action === "remove") return { removed: remove(list), list: items().map((entry) => entry.text) };
  return { list: items().map((entry) => entry.text) };
}

// Виджет на столе пульта.
export function panelList() {
  return { items: items() };
}

export function panelAdd({ text }) {
  const { full } = add(split(text));
  return full ? { items: items(), error: FULL } : { items: items() };
}

export function panelRemove({ id }) {
  remove([String(id ?? "")]);
  return { items: items() };
}

export function panelClear() {
  save([]);
  return { items: [] };
}
