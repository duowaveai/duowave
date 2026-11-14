// __TITLE__: фото дня из папки «Фото» домашней папки DuoWave — большая карточка на столе — и окошко со всеми фото
// (docs/plans/shared-folders.md, docs/plugins/AUTHORING.md, «Папки»). Право — связь `call:files/home/Фото/*`: читать
// «Фото» со всем, что в ней; человек скажет «да» один раз. Картинку на экран даёт сам шлюз (`picture: {address}` в
// карточке, `useFolderFile` в окошке) — уменьшенной, код фото не читает.
//
// Имена вслух — только с `speak` папки (`host.files.stat`): голос Алисы озвучивает Яндекс, без галочки имена не
// уходят из дома. Карточка ничего не пишет в kv: пишут функции нажатий.

const FOLDER = "files/home/Фото";
const DEPTH = 2;
const MOST = 2000;

// Все фото папки, с вложенными до двух уровней: «Фото/Отпуск 2025/1.jpg».
async function pictures() {
  const found = [];
  const walk = async (at, depth) => {
    const page = await host.files.list(at, { limit: 500 });
    for (const item of page.items) {
      if (found.length >= MOST) return;
      if (item.kind === "image") found.push({ address: item.address, name: item.name });
      else if (item.folder && depth < DEPTH) await walk(item.address, depth + 1);
    }
  };
  await walk(FOLDER, 0);
  return found;
}

// Сегодняшнее: одно на день, «Другое» сдвигает.
function todayOf(all) {
  const day = Math.floor(Date.now() / 86_400_000);
  const shift = host.kv.get("shift") ?? 0;
  return all[(((day + shift) % all.length) + all.length) % all.length];
}

const stem = (name) => String(name ?? "").replace(/\.[^.]{1,5}$/, "");

async function speakable() {
  try {
    return (await host.files.stat(FOLDER)).speak === true;
  } catch {
    return false;
  }
}

export async function today() {
  const all = await pictures();
  // Вид (docs/plugins/cards.md, «Вид»): тёплый вечерний фон, на нём фото дня — как в рамке.
  const look = { colors: ["#2a1b3d", "#5b3a6e", "#a0526d"] };
  if (all.length === 0) return { empty: "В папке «Фото» пока нет фотографий.", look, say: "Фотографий пока нет." };
  const photo = todayOf(all);
  const speak = await speakable();
  return {
    picture: { address: photo.address, caption: stem(photo.name) },
    cap: `Фото дня · ${all.length} в папке`,
    buttons: [{ text: "Другое", tap: "next" }],
    look,
    say: speak ? `Фото дня: ${stem(photo.name)}.` : "Фото дня на экране.",
  };
}

export async function next() {
  host.kv.set("shift", (host.kv.get("shift") ?? 0) + 1);
  return today();
}

// Окошку: фото страницами по 60.
export async function page({ from = 0 } = {}) {
  const all = await pictures();
  const start = Math.max(0, Number(from) || 0);
  return { total: all.length, items: all.slice(start, start + 60) };
}

export async function todaySay() {
  const card = await today();
  return card.say;
}
