// Умный дом через подключение шлюза (Home Assistant; ключ — в шлюзе, плагин его не видит: письма
// system/smart-home по шине). Перенос голосовой части встроенного навыка (gateway/src/skills/smart_home/smart_home_skill.rs), этап 10
// плана: какое устройство названо — по имени, потом по виду.

const KINDS = [
  [/свет|ламп|люстр|подсветк|торшер|бра/, ["light", "switch"]],
  [/кондиционер|отоплени|обогрев|температур|термостат|батаре/, ["climate"]],
  [/штор|жалюзи|ворот/, ["cover"]],
  [/розетк|чайник/, ["switch"]],
  [/вентилятор/, ["fan"]],
];

function normalize(text) {
  return String(text ?? "").toLowerCase().replace(/ё/g, "е").replace(/[^\p{L}\p{N}\s]+/gu, " ").replace(/\s+/g, " ").trim();
}

const stem = (word) => (word.length > 4 ? word.slice(0, -2) : word);

function score(words, name) {
  const asked = normalize(words).split(" ").filter((word) => word.length > 1 && !["в", "на", "во", "свет"].includes(word));
  if (asked.length === 0) return 0;
  const haystack = normalize(name);
  return asked.filter((word) => haystack.includes(stem(word))).length / asked.length;
}

function pick(devices, words, fallbackKinds) {
  const kinds = KINDS.filter(([pattern]) => pattern.test(normalize(words))).flatMap(([, kinds]) => kinds);
  const wanted = kinds.length > 0 ? kinds : fallbackKinds;
  const scored = devices
    .map((device) => [score(words, device.name) + (wanted.includes(device.kind) ? 0.25 : 0), device])
    .filter(([found, device]) => found >= 0.5 || (wanted.includes(device.kind) && normalize(words).split(" ").length <= 1))
    .sort((a, b) => b[0] - a[0]);
  return scored[0]?.[1] ?? null;
}

// «Включи свет на кухне», «погаси свет».
export async function light({ place, request }) {
  const off = /^(выключи|погаси)/.test(normalize(request?.text));
  const devices = await host.bus.call("system/smart-home", "devices");
  const words = `свет ${place ?? ""}`.trim();
  const lights = devices.filter((device) => device.kind === "light" || device.kind === "switch");
  const device = pick(lights, words, ["light"]);
  if (!device) return null; // не нашла — пусть ответит встроенный навык
  await host.bus.call("system/smart-home", "act", { device: device.id, action: off ? "off" : "on" });
  return `${off ? "Выключила" : "Включила"}: ${device.name}.`;
}

// Инструмент smart_home_status.
export async function status({ words }) {
  const devices = await host.bus.call("system/smart-home", "devices");
  const shown = words ? devices.filter((device) => score(words, device.name) >= 0.5 || KINDS.some(([pattern, kinds]) => pattern.test(normalize(words)) && kinds.includes(device.kind))) : devices;
  if (shown.length === 0) return "Таких устройств нет.";
  return shown.slice(0, 40).map((device) => `${device.name} (${device.kind}): ${device.state ?? "—"}`).join("; ");
}
