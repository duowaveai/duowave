// Игры на модели шлюза (перенос gateway/src/skills/own/games_skill.rs, этап 10 плана): модель ведёт игру ответом
// `model`, ходы игрока идут обратно в плагин без «Алиса» (`continue`), пока он не скажет «хватит».

const STOP = /^(хватит|стоп|закончим|закончить|выход|выйти|надоело|не хочу( больше)?( играть)?|давай закончим|конец игры)$/;

function normalize(text) {
  return String(text ?? "").toLowerCase().replace(/ё/g, "е").replace(/[^\p{L}\p{N}\s]+/gu, " ").replace(/\s+/g, " ").trim();
}

function hostPrompt(game) {
  return [
    `Ты ведёшь голосовую игру «${game}» с одним игроком на умной колонке; всё, что ты пишешь, будет озвучено.`,
    "Говори коротко: одна-две фразы за ход, без списков и разметки. Каждый твой ход заканчивай вопросом или",
    "приглашением сделать ход. Следи за правилами и счётом сам. Игра идёт, пока игрок не скажет закончить.",
  ].join("\n");
}

const key = (request) => `game:${request?.device ?? "station"}`;

// «Давай поиграем в города».
export function start({ game, request }) {
  const chosen = String(game ?? "").trim() || "на выбор игрока: викторина, города или загадки";
  host.kv.set(key(request), chosen);
  return {
    model: { system: hostPrompt(chosen), prompt: "Начнём! Объясни правила в одной фразе и сделай первый ход.", key: "game", fresh: true },
    continue: { handler: "move" },
  };
}

// Ход игрока — без «Алиса».
export function move({ text, request }) {
  if (STOP.test(normalize(text))) {
    host.kv.delete(key(request));
    return "Хорошо, поиграем в другой раз.";
  }
  const game = host.kv.get(key(request)) ?? "на выбор игрока";
  return { model: { system: hostPrompt(game), prompt: String(text ?? ""), key: "game" }, continue: true };
}
