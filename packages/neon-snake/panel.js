// Неоновая змейка — пример окошка (docs/plugins/AUTHORING.md, «Пульт»; docs/plugins/cards.md, «Карточка или окошко»):
// то, чего карточка не умеет, — своё рисование на canvas, 60 кадров в секунду, игра стрелками, свайпами и кнопками
// на экране, звук. Доска в 3D — своя перспектива на canvas 2D, без библиотек: кубики змейки с гранями и светом на доске.
// Виджет стола рисует свою рамку сам (bare): доска медленно кружится, змейка играет сама, рядом рекорд и «Играть».
// Игра — в окне приложения (раздел с nav: "link", его открывает go). Рекорд хранит backend.js.
import { h } from "preact";
import { useEffect, useRef, useState } from "preact/hooks";
import { definePlugin, icons, t as translate, useApi, usePanelState, useT } from "@duowave/panel-sdk";

const ID = "neon-snake";

// Тексты для людей: язык пульта выбирает окошко (useT), подписи definePlugin — translate(T, key).
const T = {
  ru: {
    title: "Неоновая змейка",
    titleCaps: "НЕОНОВАЯ ЗМЕЙКА",
    hint: "Стрелки или WASD · свайп · кнопки на экране",
    play: "Играть",
    playTile: "Играть ▶",
    record: "Рекорд — {n}",
    recordShort: "рекорд {n}",
    points: "очки",
    pause: "Пауза",
    pauseBtn: "Пауза",
    resume: "Продолжить",
    pauseHint: "Пробел — пауза и дальше",
    won: "Доска твоя!",
    over: "Игра окончена",
    newRecord: "Новый рекорд!",
    again: "Ещё раз",
    soundOn: "Включить звук",
    soundOff: "Выключить звук",
    next: "Дальше",
    focus: "Нажмите на поле — и стрелки заработают",
    up: "Вверх",
    left: "Влево",
    right: "Вправо",
    down: "Вниз",
    tileCap: "Мини-игра в 3D",
    source: "игра",
  },
  en: {
    title: "Neon Snake",
    titleCaps: "NEON SNAKE",
    hint: "Arrows or WASD · swipe · on-screen buttons",
    play: "Play",
    playTile: "Play ▶",
    record: "Best — {n}",
    recordShort: "best {n}",
    points: "score",
    pause: "Paused",
    pauseBtn: "Pause",
    resume: "Resume",
    pauseHint: "Space pauses and resumes",
    won: "You filled the board!",
    over: "Game over",
    newRecord: "New best!",
    again: "Again",
    soundOn: "Sound on",
    soundOff: "Sound off",
    next: "Resume",
    focus: "Click the board to use the arrow keys",
    up: "Up",
    left: "Left",
    right: "Right",
    down: "Down",
    tileCap: "3D mini-game",
    source: "game",
  },
};
const SIZE = 13; // клеток по стороне доски в игре
const TEASER = 9; // и на плитке стола
const START_MS = 150; // шаг змейки в начале
const FASTEST_MS = 72;
const BONUS_EVERY = 6; // золотая звезда — после каждого шестого яблока
const BONUS_LIFE_MS = 7000;
const GRACE_MS = 700; // после проигрыша: случайное касание или стрелка не начинают новую партию сразу

const KEYS = {
  ArrowUp: [0, -1],
  KeyW: [0, -1],
  ArrowDown: [0, 1],
  KeyS: [0, 1],
  ArrowLeft: [-1, 0],
  KeyA: [-1, 0],
  ArrowRight: [1, 0],
  KeyD: [1, 0],
};

// ——— Игра: клетки, шаг, еда ———

const same = (a, b) => a && b && a.x === b.x && a.y === b.y;

function freeCell(n, snake, ...avoid) {
  const taken = new Set(snake.concat(avoid.filter(Boolean)).map((cell) => cell.y * n + cell.x));
  const free = [];
  for (let i = 0; i < n * n; i += 1) if (!taken.has(i)) free.push(i);
  if (free.length === 0) return null;
  const pick = free[Math.floor(Math.random() * free.length)];
  return { x: pick % n, y: Math.floor(pick / n) };
}

function newGame(n, now) {
  const mid = Math.floor(n / 2);
  const snake = [
    { x: mid, y: mid + 1 },
    { x: mid, y: mid + 2 },
    { x: mid, y: mid + 3 },
  ];
  return {
    n,
    snake,
    prev: snake.map((cell) => ({ ...cell })),
    dir: [0, -1],
    queue: [],
    food: freeCell(n, snake),
    bonus: null,
    bonusUntil: 0,
    score: 0,
    eaten: 0,
    state: "ready", // ready → play ⇄ pause → over
    overAt: 0,
    stepMs: START_MS,
    last: now,
    particles: [],
    shake: 0,
    flash: 0,
  };
}

// Поворот в очередь: два быстрых нажатия подряд не теряются, разворот назад не принимается.
function turn(game, [dx, dy]) {
  const [lx, ly] = game.queue.length ? game.queue[game.queue.length - 1] : game.dir;
  if ((dx === -lx && dy === -ly) || (dx === lx && dy === ly)) return;
  if (game.queue.length < 3) game.queue.push([dx, dy]);
}

// Один шаг змейки: "move", "eat", "bonus", "over" или "won".
function step(game, now) {
  if (game.queue.length) game.dir = game.queue.shift();
  const { n } = game;
  const head = game.snake[0];
  const next = { x: head.x + game.dir[0], y: head.y + game.dir[1] };
  const grows = same(next, game.food) || same(next, game.bonus);
  const body = grows ? game.snake : game.snake.slice(0, -1);
  if (next.x < 0 || next.y < 0 || next.x >= n || next.y >= n || body.some((cell) => same(cell, next))) {
    game.state = "over";
    game.overAt = now;
    game.shake = 1;
    return "over";
  }
  const old = game.snake;
  game.snake = [next, ...body];
  // Откуда пришёл каждый кубик: между шагами он плавно едет оттуда.
  game.prev = game.snake.map((_, i) => ({ ...old[Math.min(i, old.length - 1)] }));
  let what = "move";
  if (same(next, game.food)) {
    game.score += 1;
    game.eaten += 1;
    burst(game, next, 320, 26);
    game.food = freeCell(n, game.snake, game.bonus);
    if (game.eaten % BONUS_EVERY === 0 && !game.bonus) {
      game.bonus = freeCell(n, game.snake, game.food);
      game.bonusUntil = now + BONUS_LIFE_MS;
    }
    game.stepMs = Math.max(FASTEST_MS, START_MS - game.eaten * 3);
    what = "eat";
  } else if (same(next, game.bonus)) {
    game.score += 5;
    burst(game, next, 48, 44);
    game.bonus = null;
    game.flash = 1;
    what = "bonus";
  }
  if (game.bonus && now > game.bonusUntil) game.bonus = null;
  if (!game.food) {
    game.state = "over";
    game.overAt = now;
    return "won";
  }
  return what;
}

// Змейка сама: к еде кратчайшим, без стен и себя, с отступом от тупика. Для плитки стола.
function autopilot(game) {
  const head = game.snake[0];
  const { n } = game;
  const body = game.snake.slice(0, -1);
  const target = game.bonus ?? game.food;
  const free = (x, y) => x >= 0 && y >= 0 && x < n && y < n && !body.some((cell) => cell.x === x && cell.y === y);
  const room = (x, y) => [[0, -1], [1, 0], [0, 1], [-1, 0]].filter(([dx, dy]) => free(x + dx, y + dy)).length;
  const options = [[0, -1], [1, 0], [0, 1], [-1, 0]]
    .filter(([dx, dy]) => !(dx === -game.dir[0] && dy === -game.dir[1]))
    .filter(([dx, dy]) => free(head.x + dx, head.y + dy))
    .map(([dx, dy]) => {
      const x = head.x + dx;
      const y = head.y + dy;
      const far = target ? Math.abs(target.x - x) + Math.abs(target.y - y) : 0;
      return { dir: [dx, dy], cost: far - room(x, y) * 0.6 + Math.random() * 0.4 };
    })
    .sort((a, b) => a.cost - b.cost);
  if (options.length) game.queue = [options[0].dir];
}

function burst(game, cell, hue, count) {
  const [X, Z] = world(game.n, cell.x, cell.y);
  for (let i = 0; i < count; i += 1) {
    const angle = Math.random() * Math.PI * 2;
    const speed = 1.2 + Math.random() * 2.6;
    game.particles.push({
      x: X,
      y: 0.5,
      z: Z,
      vx: Math.cos(angle) * speed,
      vy: 2.2 + Math.random() * 3,
      vz: Math.sin(angle) * speed,
      life: 1,
      hue: hue + Math.random() * 70 - 35,
    });
  }
}

function moveParticles(game, dt) {
  for (const p of game.particles) {
    p.vy -= 9 * dt;
    p.x += p.vx * dt;
    p.y = Math.max(0.02, p.y + p.vy * dt);
    p.z += p.vz * dt;
    p.life -= dt * 1.3;
  }
  game.particles = game.particles.filter((p) => p.life > 0);
}

// ——— 3D: клетка → мир → экран ———

const world = (n, x, y) => [x - (n - 1) / 2, y - (n - 1) / 2];

// Камера над доской со стороны игрока, наклон pitch, поворот yaw вокруг оси доски.
function project(view, X, Y, Z) {
  const x = X * view.cy - Z * view.sy;
  const z = X * view.sy + Z * view.cy;
  const up = Y * view.cp - z * view.sp;
  const depth = view.dist - Y * view.sp - z * view.cp;
  const scale = view.f / depth;
  return [view.ox + x * scale, view.oy - up * scale, depth];
}

// Доска целиком в кадре: при покачивании (spin: false) или при полном обороте (spin: true).
function fit(width, height, n, spin) {
  const pitch = spin ? 0.9 : 0.98;
  const view = { cy: 1, sy: 0, cp: Math.cos(pitch), sp: Math.sin(pitch), dist: n * 1.45, f: 1, ox: 0, oy: 0 };
  const half = n / 2;
  const points = [];
  if (spin) {
    for (let k = 0; k < 16; k += 1) {
      const a = (k / 16) * Math.PI * 2;
      points.push([Math.cos(a) * half * 1.42, 0, Math.sin(a) * half * 1.42], [Math.cos(a) * half * 1.42, 1, Math.sin(a) * half * 1.42]);
    }
  } else {
    for (const [x, z] of [[-1, -1], [1, -1], [1, 1], [-1, 1]]) {
      points.push([x * half * 1.08, 1, z * half * 1.08], [x * half * 1.08, -0.6, z * half * 1.08]);
    }
  }
  let [minX, maxX, minY, maxY] = [Infinity, -Infinity, Infinity, -Infinity];
  for (const [X, Y, Z] of points) {
    const [px, py] = project(view, X, Y, Z);
    minX = Math.min(minX, px);
    maxX = Math.max(maxX, px);
    minY = Math.min(minY, py);
    maxY = Math.max(maxY, py);
  }
  const scale = Math.min((width * 0.94) / (maxX - minX), (height * 0.9) / (maxY - minY));
  view.f = scale;
  view.ox = width / 2 - (scale * (minX + maxX)) / 2;
  view.oy = height / 2 - (scale * (minY + maxY)) / 2;
  return view;
}

function polygon(ctx, points) {
  ctx.beginPath();
  ctx.moveTo(points[0][0], points[0][1]);
  for (let i = 1; i < points.length; i += 1) ctx.lineTo(points[i][0], points[i][1]);
  ctx.closePath();
}

// Грани коробки: верх, дальняя, правая, ближняя, левая — обходом наружу, так что видна грань с площадью > 0.
const FACES = [
  [4, 5, 6, 7],
  [0, 1, 5, 4],
  [1, 2, 6, 5],
  [2, 3, 7, 6],
  [3, 0, 4, 7],
];

function box(ctx, view, X, Z, wide, deep, lift, tall, colors) {
  const [a, b] = [wide / 2, deep / 2];
  const corners = [];
  for (const y of [lift, lift + tall]) {
    for (const [dx, dz] of [[-a, -b], [a, -b], [a, b], [-a, b]]) corners.push(project(view, X + dx, y, Z + dz));
  }
  for (let face = 0; face < FACES.length; face += 1) {
    const fill = face === 0 ? colors.top : face === 3 ? colors.front : colors.side;
    if (!fill) continue;
    const [p, q, r] = FACES[face].map((i) => corners[i]);
    if ((q[0] - p[0]) * (r[1] - p[1]) - (q[1] - p[1]) * (r[0] - p[0]) <= 0) continue;
    polygon(ctx, FACES[face].map((i) => corners[i]));
    ctx.fillStyle = fill;
    ctx.fill();
    if (face === 0 && colors.edge) {
      ctx.strokeStyle = colors.edge;
      ctx.lineWidth = 1;
      ctx.stroke();
    }
  }
  return corners;
}

function glow(ctx, x, y, radius, color) {
  const light = ctx.createRadialGradient(x, y, 0, x, y, radius);
  light.addColorStop(0, color);
  light.addColorStop(1, "rgba(0, 0, 0, 0)");
  ctx.fillStyle = light;
  ctx.fillRect(x - radius, y - radius, radius * 2, radius * 2);
}

function stars(count) {
  return Array.from({ length: count }, () => ({
    x: Math.random(),
    y: Math.random(),
    r: Math.random() < 0.15 ? 2 : 1,
    p: Math.random() * Math.PI * 2,
  }));
}

const PALETTES = {
  game: ["#2c0f5c", "#130a33", "#05040f"],
  tile: ["#5a1475", "#1d0b47", "#070418"],
};

// Кадр: фон со звёздами, доска с сеткой и светящейся кромкой, свет змейки на доске, кубики по глубине, еда, искры.
function draw(ctx, frame) {
  const { width: W, height: H, dpr, game, now, t, view: base, sky, palette, yaw } = frame;
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  const [inner, middle, outer] = PALETTES[palette];
  const back = ctx.createRadialGradient(W * 0.5, H * 0.3, 8, W * 0.5, H * 0.55, Math.max(W, H) * 0.85);
  back.addColorStop(0, inner);
  back.addColorStop(0.55, middle);
  back.addColorStop(1, outer);
  ctx.fillStyle = back;
  ctx.fillRect(0, 0, W, H);
  for (const star of sky) {
    ctx.fillStyle = `rgba(210, 225, 255, ${0.2 + 0.35 * (0.5 + 0.5 * Math.sin(now / 700 + star.p))})`;
    ctx.fillRect(star.x * W, star.y * H, star.r, star.r);
  }

  const shake = game.shake * 9;
  const view = {
    ...base,
    cy: Math.cos(yaw),
    sy: Math.sin(yaw),
    ox: base.ox + (Math.random() - 0.5) * shake,
    oy: base.oy + (Math.random() - 0.5) * shake,
  };
  const { n } = game;
  const half = n / 2;

  // Плита доски: бока, потом верх с сеткой и кромкой.
  box(ctx, view, 0, 0, n, n, -0.55, 0.55, { side: "#1a0e3d", front: "#2b1260" });
  const top = [[-half, -half], [half, -half], [half, half], [-half, half]].map(([x, z]) => project(view, x, 0, z));
  const floor = ctx.createLinearGradient(0, Math.min(...top.map((p) => p[1])), 0, Math.max(...top.map((p) => p[1])));
  floor.addColorStop(0, "#0c0a2c");
  floor.addColorStop(1, "#22104d");
  polygon(ctx, top);
  ctx.fillStyle = floor;
  ctx.fill();
  ctx.strokeStyle = "rgba(130, 210, 255, 0.13)";
  ctx.lineWidth = 1;
  ctx.beginPath();
  for (let i = 1; i < n; i += 1) {
    const k = i - half;
    for (const [from, to] of [
      [project(view, k, 0, -half), project(view, k, 0, half)],
      [project(view, -half, 0, k), project(view, half, 0, k)],
    ]) {
      ctx.moveTo(from[0], from[1]);
      ctx.lineTo(to[0], to[1]);
    }
  }
  ctx.stroke();
  const rim = (now / 30) % 360;
  ctx.save();
  ctx.shadowColor = `hsl(${rim}, 100%, 60%)`;
  ctx.shadowBlur = 16;
  ctx.strokeStyle = `hsl(${rim}, 100%, 66%)`;
  ctx.lineWidth = 2;
  polygon(ctx, top);
  ctx.stroke();
  ctx.restore();

  const dead = game.state === "over";
  const at = (i) => {
    const cell = game.snake[i];
    const from = game.prev[i] ?? cell;
    return world(n, from.x + (cell.x - from.x) * t, from.y + (cell.y - from.y) * t);
  };
  const hueOf = (i) => (now * 0.08 + i * 26) % 360;

  // Свет змейки и еды на доске.
  ctx.globalCompositeOperation = "lighter";
  for (let i = 0; i < game.snake.length; i += 1) {
    const [X, Z] = at(i);
    const [px, py, depth] = project(view, X, 0.01, Z);
    glow(ctx, px, py, (view.f * 1.15) / depth, dead ? "rgba(255, 60, 90, 0.2)" : `hsla(${hueOf(i)}, 100%, 60%, 0.42)`);
  }
  const bob = Math.sin(now / 260) * 0.12;
  if (game.food) {
    const [X, Z] = world(n, game.food.x, game.food.y);
    const [px, py, depth] = project(view, X, 0.01, Z);
    glow(ctx, px, py, (view.f * (1.1 + bob)) / depth, "rgba(255, 70, 190, 0.32)");
  }
  ctx.globalCompositeOperation = "source-over";

  // Всё объёмное — от дальнего к ближнему.
  const things = [];
  for (let i = 0; i < game.snake.length; i += 1) {
    const [X, Z] = at(i);
    things.push({ kind: "cube", i, X, Z, depth: project(view, X, 0.4, Z)[2] });
  }
  if (game.food) {
    const [X, Z] = world(n, game.food.x, game.food.y);
    things.push({ kind: "food", X, Z, depth: project(view, X, 0.45, Z)[2] });
  }
  if (game.bonus) {
    const [X, Z] = world(n, game.bonus.x, game.bonus.y);
    things.push({ kind: "bonus", X, Z, depth: project(view, X, 0.6, Z)[2] });
  }
  things.sort((a, b) => b.depth - a.depth);
  for (const thing of things) {
    if (thing.kind === "cube") {
      const { i, X, Z } = thing;
      const head = i === 0;
      const hue = hueOf(i);
      const ripple = 0.04 + 0.05 * Math.sin(now / 170 - i * 0.7);
      const size = head ? 0.9 : 0.82 - Math.min(0.2, i * 0.004);
      const colors = dead
        ? { top: `hsl(${hue}, 18%, 48%)`, side: `hsl(${hue}, 15%, 30%)`, front: `hsl(${hue}, 15%, 24%)`, edge: "rgba(255,255,255,0.2)" }
        : {
            top: `hsl(${hue}, 100%, ${head ? 74 : 64}%)`,
            side: `hsl(${hue}, 95%, 46%)`,
            front: `hsl(${hue}, 90%, 36%)`,
            edge: head ? "rgba(255, 255, 255, 0.9)" : "rgba(255, 255, 255, 0.5)",
          };
      const tall = head ? 0.8 : 0.7;
      box(ctx, view, X, Z, size, size, ripple, tall, colors);
      if (head) eyes(ctx, view, X, Z, ripple + tall, game.dir, dead);
    } else if (thing.kind === "food") {
      const [sx, sy, depth] = project(view, thing.X, 0.01, thing.Z);
      const shadow = (view.f * 0.3) / depth;
      ctx.fillStyle = "rgba(0, 0, 0, 0.35)";
      ctx.beginPath();
      ctx.ellipse(sx, sy, shadow, shadow * view.sp * 0.55 + 1, 0, 0, Math.PI * 2);
      ctx.fill();
      const [px, py] = project(view, thing.X, 0.5 + bob, thing.Z);
      const r = (view.f * 0.32) / depth;
      ctx.globalCompositeOperation = "lighter";
      glow(ctx, px, py, r * 2.6, "rgba(255, 60, 170, 0.45)");
      ctx.globalCompositeOperation = "source-over";
      const orb = ctx.createRadialGradient(px - r * 0.35, py - r * 0.4, r * 0.1, px, py, r);
      orb.addColorStop(0, "#ffffff");
      orb.addColorStop(0.35, "#ff8ad8");
      orb.addColorStop(1, "#d6157f");
      ctx.fillStyle = orb;
      ctx.beginPath();
      ctx.arc(px, py, r, 0, Math.PI * 2);
      ctx.fill();
    } else {
      star(ctx, view, thing.X, thing.Z, now, game.bonusUntil - now);
    }
  }

  // Искры.
  ctx.globalCompositeOperation = "lighter";
  for (const p of game.particles) {
    const [px, py, depth] = project(view, p.x, p.y, p.z);
    const r = Math.max(1, (view.f * 0.07) / depth);
    ctx.fillStyle = `hsla(${p.hue}, 100%, 66%, ${p.life})`;
    ctx.beginPath();
    ctx.arc(px, py, r, 0, Math.PI * 2);
    ctx.fill();
  }
  ctx.globalCompositeOperation = "source-over";
  if (game.flash > 0) {
    ctx.fillStyle = `rgba(255, 210, 90, ${game.flash * 0.22})`;
    ctx.fillRect(0, 0, W, H);
  }
  if (dead && game.shake > 0) {
    ctx.fillStyle = `rgba(255, 40, 80, ${game.shake * 0.25})`;
    ctx.fillRect(0, 0, W, H);
  }
}

function eyes(ctx, view, X, Z, top, [dx, dy], dead) {
  const side = [-dy, dx];
  for (const k of [-1, 1]) {
    const ex = X + dx * 0.2 + side[0] * 0.19 * k;
    const ez = Z + dy * 0.2 + side[1] * 0.19 * k;
    const [px, py, depth] = project(view, ex, top + 0.002, ez);
    const r = (view.f * 0.1) / depth;
    ctx.fillStyle = "#ffffff";
    ctx.beginPath();
    ctx.arc(px, py, r, 0, Math.PI * 2);
    ctx.fill();
    if (dead) {
      ctx.strokeStyle = "#1a0b2e";
      ctx.lineWidth = Math.max(1, r * 0.35);
      ctx.beginPath();
      ctx.moveTo(px - r * 0.6, py - r * 0.6);
      ctx.lineTo(px + r * 0.6, py + r * 0.6);
      ctx.moveTo(px + r * 0.6, py - r * 0.6);
      ctx.lineTo(px - r * 0.6, py + r * 0.6);
      ctx.stroke();
      continue;
    }
    const [lx, ly] = project(view, ex + dx * 0.05, top + 0.002, ez + dy * 0.05);
    ctx.fillStyle = "#1a0b2e";
    ctx.beginPath();
    ctx.arc(lx, ly, r * 0.52, 0, Math.PI * 2);
    ctx.fill();
  }
}

function star(ctx, view, X, Z, now, left) {
  const [px, py, depth] = project(view, X, 0.62 + Math.sin(now / 200) * 0.1, Z);
  const r = (view.f * 0.42) / depth;
  ctx.globalCompositeOperation = "lighter";
  glow(ctx, px, py, r * 2.4, "rgba(255, 200, 60, 0.5)");
  ctx.globalCompositeOperation = "source-over";
  const spin = now / 500;
  ctx.beginPath();
  for (let k = 0; k < 10; k += 1) {
    const a = spin + (k * Math.PI) / 5;
    const radius = k % 2 === 0 ? r : r * 0.45;
    const x = px + Math.cos(a) * radius;
    const y = py + Math.sin(a) * radius * 0.85;
    if (k === 0) ctx.moveTo(x, y);
    else ctx.lineTo(x, y);
  }
  ctx.closePath();
  const gold = ctx.createLinearGradient(px, py - r, px, py + r);
  gold.addColorStop(0, "#fff6b0");
  gold.addColorStop(1, "#ff9d00");
  ctx.fillStyle = gold;
  ctx.fill();
  // Сколько звезде осталось — дугой вокруг.
  const share = Math.max(0, Math.min(1, left / BONUS_LIFE_MS));
  ctx.strokeStyle = "rgba(255, 230, 140, 0.8)";
  ctx.lineWidth = 2;
  ctx.beginPath();
  ctx.arc(px, py, r * 1.45, -Math.PI / 2, -Math.PI / 2 + share * Math.PI * 2);
  ctx.stroke();
}

// ——— Окно: холст по ширине, цикл кадров, звук ———

function useCanvas(ratio, min, max) {
  const wrap = useRef(null);
  const canvas = useRef(null);
  const [size, setSize] = useState(null);
  useEffect(() => {
    const element = wrap.current;
    if (!element) return undefined;
    const measure = () => {
      const width = Math.round(element.clientWidth);
      if (!width) return;
      const height = Math.round(Math.max(min, Math.min(max, width * ratio)));
      const dpr = Math.min(2, window.devicePixelRatio || 1);
      setSize((was) => (was && was.width === width && was.height === height && was.dpr === dpr ? was : { width, height, dpr }));
    };
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(element);
    return () => observer.disconnect();
  }, []);
  useEffect(() => {
    const element = canvas.current;
    if (!element || !size) return;
    element.width = Math.round(size.width * size.dpr);
    element.height = Math.round(size.height * size.dpr);
    element.style.width = `${size.width}px`;
    element.style.height = `${size.height}px`;
  }, [size]);
  return { wrap, canvas, size };
}

// Кадры, пока окно на экране; fps — потолок (плитке стола хватит 30).
function useFrames(paint, fps) {
  const latest = useRef(paint);
  latest.current = paint;
  useEffect(() => {
    let frame = 0;
    let last = 0;
    const gap = 1000 / fps - 2;
    const run = (now) => {
      frame = requestAnimationFrame(run);
      if (document.hidden || now - last < gap) return;
      const dt = last ? Math.min(0.05, (now - last) / 1000) : 0;
      last = now;
      latest.current(now, dt);
    };
    frame = requestAnimationFrame(run);
    return () => cancelAnimationFrame(frame);
  }, [fps]);
}

const SCALE = [0, 2, 4, 7, 9, 12, 14, 16];

function tone(audio, frequency, at, length, type, volume) {
  const oscillator = audio.createOscillator();
  const gain = audio.createGain();
  oscillator.type = type;
  oscillator.frequency.setValueAtTime(frequency, at);
  gain.gain.setValueAtTime(0.0001, at);
  gain.gain.exponentialRampToValueAtTime(volume, at + 0.012);
  gain.gain.exponentialRampToValueAtTime(0.0001, at + length);
  oscillator.connect(gain).connect(audio.destination);
  oscillator.start(at);
  oscillator.stop(at + length + 0.03);
}

// Короткие звуки синтезом: яблоко — нота выше с каждым съеденным, звезда — арпеджио, конец — вниз.
function useSound(muted) {
  const audio = useRef(null);
  return (kind, count = 0) => {
    if (muted.current) return;
    try {
      const Context = window.AudioContext || window.webkitAudioContext;
      if (!Context) return;
      audio.current = audio.current ?? new Context();
      const context = audio.current;
      if (context.state === "suspended") context.resume();
      const at = context.currentTime + 0.01;
      const note = (semitones) => 523.25 * Math.pow(2, semitones / 12);
      if (kind === "eat") {
        const base = SCALE[count % SCALE.length];
        tone(context, note(base), at, 0.12, "triangle", 0.09);
        tone(context, note(base + 7), at + 0.06, 0.14, "triangle", 0.07);
      } else if (kind === "bonus") {
        [0, 4, 7, 12, 16].forEach((s, i) => tone(context, note(s), at + i * 0.06, 0.16, "square", 0.05));
      } else if (kind === "start") {
        [0, 7, 12].forEach((s, i) => tone(context, note(s), at + i * 0.08, 0.14, "triangle", 0.07));
      } else if (kind === "over") {
        [-5, -9, -14].forEach((s, i) => tone(context, note(s), at + i * 0.12, 0.26, "sawtooth", 0.05));
      }
    } catch {
      // без звука
    }
  };
}

function Game() {
  const tr = useT(T);
  const api = useApi(ID);
  const { wrap, canvas, size } = useCanvas(0.66, 300, 640);
  const game = useRef(null);
  if (!game.current) game.current = newGame(SIZE, performance.now());
  const sky = useRef(null);
  if (!sky.current) sky.current = stars(90);
  const muted = useRef(false);
  const sound = useSound(muted);
  const [hud, setHud] = useState({ score: 0, state: "ready", record: false, won: false });
  const [best, setBest] = useState(0);
  const [quiet, setQuiet] = useState(false);
  const [focused, setFocused] = useState(true);
  const touch = useRef(null);

  const sync = (extra = {}) => {
    const g = game.current;
    setHud((was) =>
      was.score === g.score && was.state === g.state && Object.keys(extra).every((key) => was[key] === extra[key])
        ? was
        : { ...was, score: g.score, state: g.state, ...extra },
    );
  };

  useEffect(() => {
    api.call("best").then((stats) => setBest(stats?.best ?? 0)).catch(() => {});
  }, [api]);

  const start = () => {
    const now = performance.now();
    if (game.current.state === "over" && now - game.current.overAt < GRACE_MS) return false;
    if (game.current.state === "over") game.current = newGame(SIZE, now);
    game.current.state = "play";
    game.current.last = now;
    sound("start");
    sync({ record: false, won: false });
    return true;
  };
  const pause = () => {
    const g = game.current;
    if (g.state === "play") g.state = "pause";
    else if (g.state === "pause") {
      g.state = "play";
      g.last = performance.now();
    }
    sync();
  };
  const press = (dir) => {
    const g = game.current;
    if ((g.state === "ready" || g.state === "over") && !start()) return;
    if (g.state === "pause") pause();
    turn(game.current, dir);
  };
  const finish = (points, won) => {
    sync({ won });
    api
      .call("score", { points })
      .then((stats) => {
        setBest(stats?.best ?? points);
        sync({ record: Boolean(stats?.record), won });
      })
      .catch(() => {});
  };

  // Клавиши слышит окошко, когда в фокусе: стрелки и WASD, пробел — пауза, Enter — играть.
  const keys = useRef(null);
  keys.current = { press, pause, start };
  useEffect(() => {
    const onKey = (event) => {
      const dir = KEYS[event.code] ?? KEYS[event.key];
      const state = game.current.state;
      if (dir) {
        event.preventDefault();
        keys.current.press(dir);
      } else if (event.code === "Space") {
        event.preventDefault();
        if (state === "play" || state === "pause") keys.current.pause();
        else keys.current.start();
      } else if (event.key === "Enter" && state !== "play") {
        event.preventDefault();
        if (state === "pause") keys.current.pause();
        else keys.current.start();
      }
    };
    const onFocus = () => setFocused(true);
    const onBlur = () => {
      setFocused(false);
      if (game.current.state === "play") keys.current.pause();
    };
    window.addEventListener("keydown", onKey);
    window.addEventListener("focus", onFocus);
    window.addEventListener("blur", onBlur);
    setFocused(document.hasFocus());
    return () => {
      window.removeEventListener("keydown", onKey);
      window.removeEventListener("focus", onFocus);
      window.removeEventListener("blur", onBlur);
    };
  }, []);

  const views = useRef({ key: "", view: null });
  useFrames((now, dt) => {
    const element = canvas.current;
    if (!element || !size) return;
    const g = game.current;
    if (g.state === "play") {
      let steps = 0;
      while (g.state === "play" && now - g.last >= g.stepMs && steps < 3) {
        g.last += g.stepMs;
        steps += 1;
        const what = step(g, now);
        if (what === "eat") sound("eat", g.eaten);
        if (what === "bonus") sound("bonus");
        if (what === "over" || what === "won") {
          sound("over");
          finish(g.score, what === "won");
        }
        sync();
      }
      if (now - g.last > g.stepMs * 3) g.last = now;
    }
    moveParticles(g, dt);
    g.shake = Math.max(0, g.shake - dt * 2.4);
    g.flash = Math.max(0, g.flash - dt * 2.5);
    const key = `${size.width}x${size.height}`;
    if (views.current.key !== key) views.current = { key, view: fit(size.width, size.height, SIZE, false) };
    const t = g.state === "play" ? Math.min(1, Math.max(0, (now - g.last) / g.stepMs)) : 1;
    draw(element.getContext("2d"), {
      ...size,
      game: g,
      now,
      t,
      view: views.current.view,
      sky: sky.current,
      palette: "game",
      yaw: 0.13 * Math.sin(now / 3800),
    });
  }, 60);

  const onPointerDown = (event) => {
    touch.current = { x: event.clientX, y: event.clientY };
    wrap.current?.focus();
  };
  const onPointerUp = (event) => {
    const from = touch.current;
    touch.current = null;
    if (!from) return;
    const dx = event.clientX - from.x;
    const dy = event.clientY - from.y;
    if (Math.max(Math.abs(dx), Math.abs(dy)) < 26) {
      const state = game.current.state;
      if (state === "ready" || state === "over") start();
      else if (state === "pause") pause();
      return;
    }
    press(Math.abs(dx) > Math.abs(dy) ? [Math.sign(dx), 0] : [0, Math.sign(dy)]);
  };

  const pad = (dir, label, area) =>
    h(
      "button",
      {
        type: "button",
        class: "ns-pad-key",
        style: { gridArea: area },
        "aria-label": label,
        onPointerDown: (event) => {
          event.preventDefault();
          event.stopPropagation();
          press(dir);
        },
      },
      { up: "▲", down: "▼", left: "◀", right: "▶" }[area],
    );

  const overlay =
    hud.state === "ready"
      ? h(
          "div",
          { class: "ns-overlay" },
          h("div", { class: "ns-title" }, tr("titleCaps")),
          h("div", { class: "ns-hint" }, tr("hint")),
          h("button", { type: "button", class: "ns-go", onClick: start }, tr("play")),
          best > 0 && h("div", { class: "ns-hint" }, tr("record", { n: best })),
        )
      : hud.state === "pause"
        ? h(
            "div",
            { class: "ns-overlay" },
            h("div", { class: "ns-title ns-small" }, tr("pause")),
            h("button", { type: "button", class: "ns-go", onClick: pause }, tr("resume")),
            h("div", { class: "ns-hint" }, tr("pauseHint")),
          )
        : hud.state === "over"
          ? h(
              "div",
              { class: "ns-overlay" },
              h("div", { class: "ns-title ns-small" }, hud.won ? tr("won") : tr("over")),
              h("div", { class: "ns-score-big" }, String(hud.score)),
              h("div", { class: hud.record ? "ns-record" : "ns-hint" }, hud.record ? tr("newRecord") : tr("record", { n: best })),
              h("button", { type: "button", class: "ns-go", onClick: start }, tr("again")),
            )
          : null;

  return h(
    "div",
    {
      class: "ns-game",
      ref: wrap,
      tabIndex: 0,
      onPointerDown,
      onPointerUp,
    },
    h("canvas", { ref: canvas, class: "ns-canvas" }),
    h(
      "div",
      { class: "ns-hud" },
      h("div", { class: "ns-score" }, h("b", null, String(hud.score)), h("span", null, best ? tr("recordShort", { n: best }) : tr("points"))),
      h(
        "div",
        { class: "ns-tools" },
        h(
          "button",
          {
            type: "button",
            class: "ns-tool",
            "aria-label": quiet ? tr("soundOn") : tr("soundOff"),
            onPointerDown: (event) => event.stopPropagation(),
            onClick: () => {
              muted.current = !muted.current;
              setQuiet(muted.current);
            },
          },
          quiet ? "🔇" : "🔊",
        ),
        (hud.state === "play" || hud.state === "pause") &&
          h(
            "button",
            {
              type: "button",
              class: "ns-tool",
              "aria-label": hud.state === "play" ? tr("pauseBtn") : tr("next"),
              onPointerDown: (event) => event.stopPropagation(),
              onClick: pause,
            },
            hud.state === "play" ? "❚❚" : "▶",
          ),
      ),
    ),
    overlay,
    !focused && hud.state !== "over" && h("div", { class: "ns-focus" }, tr("focus")),
    h(
      "div",
      { class: "ns-pad", onPointerDown: (event) => event.stopPropagation() },
      pad([0, -1], tr("up"), "up"),
      pad([-1, 0], tr("left"), "left"),
      pad([1, 0], tr("right"), "right"),
      pad([0, 1], tr("down"), "down"),
    ),
  );
}

// Плитка стола: доска кружится, змейка играет сама; рекорд и «Играть» поверх.
function Tile() {
  const tr = useT(T);
  const api = useApi(ID);
  const panel = usePanelState();
  const { wrap, canvas, size } = useCanvas(0.62, 160, 230);
  const [stats, setStats] = useState(null);
  const game = useRef(null);
  if (!game.current) {
    game.current = newGame(TEASER, performance.now());
    game.current.state = "play";
  }
  const sky = useRef(null);
  if (!sky.current) sky.current = stars(40);
  const views = useRef({ key: "", view: null });
  const rest = useRef(0);

  useEffect(() => {
    api.call("best").then(setStats).catch(() => setStats(null));
  }, [api, panel.version]);

  useFrames((now, dt) => {
    const element = canvas.current;
    if (!element || !size) return;
    const g = game.current;
    if (g.state === "over") {
      rest.current += dt;
      if (rest.current > 1.2) {
        rest.current = 0;
        game.current = newGame(TEASER, now);
        game.current.state = "play";
      }
    } else {
      while (now - g.last >= 170 && g.state === "play") {
        g.last += 170;
        autopilot(g);
        step(g, now);
      }
      if (now - g.last > 600) g.last = now;
    }
    moveParticles(game.current, dt);
    game.current.shake = Math.max(0, game.current.shake - dt * 2.4);
    game.current.flash = 0;
    const key = `${size.width}x${size.height}`;
    if (views.current.key !== key) views.current = { key, view: fit(size.width, size.height, TEASER, true) };
    const current = game.current;
    const t = current.state === "play" ? Math.min(1, Math.max(0, (now - current.last) / 170)) : 1;
    draw(element.getContext("2d"), {
      ...size,
      game: current,
      now,
      t,
      view: views.current.view,
      sky: sky.current,
      palette: "tile",
      yaw: now / 7000,
    });
  }, 30);

  return h(
    "div",
    { class: "widget ns-tile", ref: wrap },
    h("canvas", { ref: canvas, class: "ns-canvas" }),
    h(
      "div",
      { class: "ns-tile-body" },
      h("div", { class: "ns-tile-title" }, tr("title")),
      h("div", { class: "ns-tile-cap" }, stats?.best ? tr("record", { n: stats.best }) : tr("tileCap")),
      h("button", { type: "button", class: "ns-go ns-go-small", onClick: () => panel.go(ID) }, tr("playTile")),
    ),
  );
}

export default definePlugin({
  id: ID,
  sections: [{ id: ID, title: translate(T, "title"), icon: icons.sparkle, Screen: Game, nav: "link" }],
  slots: {
    "desk.widgets": [
      { id: "tile", title: translate(T, "title"), icon: icons.sparkle, size: "small", initial: true, source: translate(T, "source"), bare: true, Component: Tile },
    ],
  },
});
