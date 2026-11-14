// Неоновая змейка: рекорд и сыгранные партии. `best` только читает — его зовёт виджет стола всякий раз, как данные
// приложения поменялись; пишет одна `score`, в конце партии.

const EMPTY = { best: 0, games: 0, last: 0 };

export function best() {
  return host.kv.get("stats") ?? EMPTY;
}

export function score({ points } = {}) {
  const was = host.kv.get("stats") ?? EMPTY;
  const value = Math.max(0, Math.min(100000, Math.floor(Number(points) || 0)));
  const now = { best: Math.max(was.best, value), games: was.games + 1, last: value };
  host.kv.set("stats", now);
  return { ...now, record: value > was.best };
}
