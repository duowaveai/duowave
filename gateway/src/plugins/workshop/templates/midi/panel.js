// __TITLE__: виджет на столе и окно урока. Клавиатуру слышит useMidi (шлюз на компьютере; в «Примерке» — клавиатура
// на экране), звук — useSynth, клавиши — PianoKeys (docs/plugins/AUTHORING.md, «MIDI-клавиатура»). Урок «ждёт тебя»:
// темпа нет, следующая нота светится, пока её не нажмут. Прогресс сохраняется одним вызовом в конце, не на каждую
// клавишу.
import { h } from "preact";
import { useEffect, useMemo, useRef, useState } from "preact/hooks";
import { PianoKeys, definePlugin, icons, noteName, readMidiFile, melodyOf, useApi, useMidi, usePanelState, useSynth } from "@duowave/panel-sdk";

const ID = "__ID__";
const SIZES = [25, 32, 37, 49, 61, 76, 88];
const LOWEST = { 25: 48, 32: 53, 37: 48, 49: 36, 61: 36, 76: 28, 88: 21 };

// Сколько клавиш, по названию: «Arturia KeyStep 37», «Keystation 49 MK3». Подсказка, выбирает человек.
const sizeByName = (name) => SIZES.find((keys) => new RegExp(`(^|\\D)${keys}(\\D|$)`).test(name ?? ""));

// Данные учителя: мелодии, выбранная, прогресс, клавиатуры. Перечитываются, когда данные приложения поменялись.
function useTeacher() {
  const api = useApi(ID);
  const { version } = usePanelState();
  const [data, setData] = useState(null);
  const [error, setError] = useState(null);
  useEffect(() => {
    api.call("state").then(setData).catch((e) => setError(e?.message ?? String(e)));
  }, [api, version]);
  return { api, data, error };
}

// Где клавиатура и куда сдвинуть мелодию, чтобы влезла: сдвиг на октавы, крайние ноты — переносом.
function layout(melody, board) {
  const notes = melody?.notes ?? [];
  const pitches = notes.map((note) => note[0]);
  const min = Math.min(...pitches, 60);
  const max = Math.max(...pitches, 60);
  let from;
  let to;
  if (board) {
    from = board.low ?? LOWEST[board.keys] ?? 36;
    to = from + (board.keys ?? 61) - 1;
  } else {
    from = 12 * Math.floor(min / 12);
    to = Math.max(from + 24, 12 * Math.ceil((max + 1) / 12));
  }
  let shift = 0;
  let best = Infinity;
  for (let k = -5; k <= 5; k += 1) {
    const outside = pitches.filter((p) => p + 12 * k < from || p + 12 * k > to).length;
    if (outside < best || (outside === best && Math.abs(k) < Math.abs(shift / 12))) {
      best = outside;
      shift = 12 * k;
    }
  }
  const fold = (p) => {
    let q = p + shift;
    while (q < from) q += 12;
    while (q > to) q -= 12;
    return q;
  };
  return { from, to, notes: notes.map(([note, at, length]) => [fold(note), at, length]), folded: best };
}

// Виджет клавиатуру не слушает: она открыта, только пока открыто окно урока.
function Widget() {
  const { data, error } = useTeacher();
  const panel = usePanelState();
  if (error) return h("p", { class: "w-cap m-0" }, error);
  if (!data) return h("p", { class: "w-cap m-0" }, "Читаю…");
  const melody = data.melodies.find((m) => m.id === data.current) ?? data.melodies[0];
  const progress = data.progress[melody?.id];
  const first = melody?.notes[0]?.[0] ?? 60;
  const from = 12 * Math.floor(first / 12);
  return h(
    "div",
    { class: "flex flex-col gap-6" },
    h("span", { class: "w-big" }, melody?.title ?? "Мелодий нет"),
    h("span", { class: "w-cap" }, progress ? `Пройдена ${progress.times} раз · лучший раз — ошибок ${progress.best}` : "Ещё не пройдена"),
    h(PianoKeys, { from, to: from + 12, next: [first], names: false, height: 44 }),
    h("button", { class: "w-btn", type: "button", onClick: () => panel.go(ID) }, "Открыть урок"),
  );
}

function Lesson() {
  const { api, data, error } = useTeacher();
  const midi = useMidi();
  const synth = useSynth();
  const [index, setIndex] = useState(0);
  const [mistakes, setMistakes] = useState(0);
  const [wrong, setWrong] = useState(null);
  const [down, setDown] = useState([]);
  const [playing, setPlaying] = useState(null);
  const [setup, setSetup] = useState(null);
  const [note, setNote] = useState(null);
  const [tracks, setTracks] = useState(null);
  const file = useRef(null);

  const melody = data ? data.melodies.find((m) => m.id === data.current) ?? data.melodies[0] : null;
  const deviceName = midi.state === "ready" && midi.via !== "example" ? midi.input?.name : null;
  const board = deviceName ? data?.keyboards?.[deviceName] : null;
  const fitted = useMemo(() => layout(melody, board?.keys ? board : null), [melody, board]);
  const target = fitted.notes[index]?.[0];
  const finished = melody && index >= fitted.notes.length;
  const sounds = board?.sounds === true;

  // Свежее — для обработчика нот: он живёт, пока жив канал, а не до перерисовки.
  const live = useRef({});
  live.current = { target, finished, setup, sounds, index };

  // Первый раз с этой клавиатурой — три шага: сколько клавиш, самая левая, звучит ли сама.
  useEffect(() => {
    if (deviceName && data && !board?.keys && !setup) setSetup({ step: "keys" });
  }, [deviceName, data, board]);

  useEffect(
    () =>
      midi.on((e) => {
        const now = live.current;
        if (e.type === "pedal") return synth.pedal(e.down);
        if (e.type === "off") {
          if (!(now.sounds && e.from === "keyboard")) synth.up(e.note);
          setDown((keys) => keys.filter((key) => key !== e.note));
          return;
        }
        if (e.type !== "on") return;
        if (now.setup?.step === "low" && e.from === "keyboard") {
          api.call("keyboard", { name: deviceName, low: e.note }).then(() => setSetup({ step: "sounds" }));
          return;
        }
        if (!(now.sounds && e.from === "keyboard")) synth.down(e.note, e.velocity);
        setDown((keys) => [...keys.filter((key) => key !== e.note), e.note]);
        if (now.finished || now.target == null) return;
        if (e.note === now.target) {
          setWrong(null);
          setIndex((i) => i + 1);
        } else {
          setWrong(e.note);
          setMistakes((m) => m + 1);
          setTimeout(() => setWrong((w) => (w === e.note ? null : w)), 300);
        }
      }),
    [midi, synth, api, deviceName],
  );

  // Дошли до конца — прогресс одним вызовом.
  useEffect(() => {
    if (finished && melody) api.call("done", { id: melody.id, mistakes }).catch(() => {});
  }, [finished]);

  const restart = () => {
    setIndex(0);
    setMistakes(0);
    setWrong(null);
  };
  useEffect(restart, [melody?.id]);

  const listen = () => {
    if (playing) {
      synth.stop();
      midi.stop();
      setPlaying(null);
      return;
    }
    const tempo = melody.tempo ?? 100;
    if (sounds && midi.output) {
      const beat = 60000 / tempo;
      midi.play(fitted.notes.map(([n, at, length]) => [n, 90, Math.round(at * beat), Math.round(length * beat * 0.95)]), { id: "listen" });
      const end = fitted.notes.reduce((last, [, at, length]) => Math.max(last, at + length), 0) * beat;
      setPlaying(0);
      setTimeout(() => setPlaying(null), end);
      return;
    }
    synth.playAll(fitted.notes, { tempo, onNote: (i) => setPlaying(i), onEnd: () => setPlaying(null) });
    setPlaying(0);
  };

  const pick = async (event) => {
    const chosen = event.currentTarget.files?.[0];
    event.currentTarget.value = "";
    if (!chosen) return;
    try {
      const song = await readMidiFile(chosen);
      const usable = song.tracks.filter((track) => !track.drums && track.melody.length > 0);
      if (usable.length === 0) throw new Error("В файле нет мелодии: только ударные.");
      if (usable.length === 1) return save(chosen.name, song.tempo, usable[0]);
      setTracks({ name: chosen.name, tempo: song.tempo, list: usable });
    } catch (e) {
      setNote(e?.message ?? String(e));
    }
  };
  const save = (name, tempo, track) => {
    setTracks(null);
    const title = name.replace(/\.midi?$/i, "") + (track.name ? ` · ${track.name}` : "");
    api.call("add", { title, tempo, notes: melodyOf(track) }).then(() => setNote(`Мелодия «${title}» добавлена.`)).catch((e) => setNote(e?.message ?? String(e)));
  };

  if (error) return h("p", { class: "t-caption" }, error);
  if (!data) return h("p", { class: "t-caption" }, "Читаю…");

  const guess = sizeByName(deviceName);
  const steps =
    setup &&
    h(
      "div",
      { class: "flex flex-col gap-6" },
      setup.step === "keys" && [
        h("span", null, `Сколько клавиш у «${deviceName}»?${guess ? ` По названию — ${guess}.` : ""}`),
        h("div", { class: "flex gap-6", style: { flexWrap: "wrap" } }, ...SIZES.map((keys) =>
          h("button", { class: keys === guess ? "pill pill-own" : "pill", type: "button", onClick: () => api.call("keyboard", { name: deviceName, keys }).then(() => setSetup({ step: "low" })) }, String(keys)))),
      ],
      setup.step === "low" && h("span", null, "Нажмите на клавиатуре самую левую клавишу."),
      setup.step === "sounds" && [
        h("span", null, "Клавиатура звучит сама, без компьютера?"),
        h("div", { class: "flex gap-6" },
          h("button", { class: "pill", type: "button", onClick: () => api.call("keyboard", { name: deviceName, sounds: true }).then(() => setSetup(null)) }, "Да, у неё свой звук"),
          h("button", { class: "pill", type: "button", onClick: () => api.call("keyboard", { name: deviceName, sounds: false }).then(() => setSetup(null)) }, "Нет, звук — из пульта")),
      ],
    );

  return h(
    "div",
    { class: "flex flex-col gap-12" },
    h(
      "div",
      { class: "flex gap-6", style: { flexWrap: "wrap", alignItems: "center" } },
      h("select", { value: melody?.id, onChange: (e) => api.call("choose", { id: e.currentTarget.value }) },
        ...data.melodies.map((m) => h("option", { value: m.id }, `${m.title} — ${m.level}`))),
      h("button", { class: "pill", type: "button", onClick: () => file.current?.click() }, "Своя мелодия…"),
      melody?.own && h("button", { class: "pill", type: "button", onClick: () => api.call("remove", { id: melody.id }) }, "Убрать"),
      h("input", { ref: file, type: "file", accept: ".mid,.midi,audio/midi", hidden: true, onChange: pick }),
    ),
    tracks && h("div", { class: "flex flex-col gap-6" },
      h("span", null, `В файле ${tracks.list.length} дорожки — какую учить?`),
      h("div", { class: "flex gap-6", style: { flexWrap: "wrap" } }, ...tracks.list.map((track) =>
        h("button", { class: "pill", type: "button", onClick: () => save(tracks.name, tracks.tempo, track) }, `${track.name ?? `Дорожка ${track.index + 1}`} · ${track.melody.length} нот`)))),
    note && h("span", { class: "t-caption" }, note),
    h(
      "div",
      { class: "flex gap-6", style: { flexWrap: "wrap", alignItems: "center" } },
      h(
        "span",
        { class: "t-caption" },
        midi.state === "ready"
          ? `${midi.input?.name} · ${midi.via === "browser" ? "через браузер" : "через компьютер"}${midi.delayMs != null && midi.delayMs > 20 ? ` · задержка ${midi.delayMs} мс` : ""}`
          : midi.why ?? "Подключаю клавиатуру…",
      ),
      // Ответили не так — три шага заново.
      board?.keys && !setup &&
        h("button", { class: "pill", type: "button", onClick: () => api.call("keyboard", { name: deviceName, reset: true }).then(() => setSetup({ step: "keys" })) }, "Клавиатура заново"),
    ),
    midi.inputs.length > 1 && h("select", { value: midi.input?.id, onChange: (e) => midi.choose(e.currentTarget.value) },
      ...midi.inputs.map((port) => h("option", { value: port.id }, port.name))),
    (midi.state === "browser" || (midi.state === "nodevice" && midi.browserPossible)) &&
      h("button", { class: "pill", type: "button", onClick: () => midi.useBrowser() }, "Подключить через браузер"),
    steps,
    !sounds && synth.needsTap && h("button", { class: "pill", type: "button", onClick: () => synth.start() }, "Начать — включить звук"),
    synth.delayMs != null && synth.delayMs > 80 && !sounds &&
      h("span", { class: "t-caption" }, `Звук идёт ${synth.delayMs} мс — наверное, наушники по Bluetooth. Играть удобнее со звуком клавиатуры или из динамиков.`),
    h(
      "span",
      null,
      finished
        ? `Готово! «${melody.title}», ошибок: ${mistakes}. Ещё раз?`
        : `«${melody?.title}» · ${index} из ${fitted.notes.length}${mistakes ? ` · ошибок ${mistakes}` : ""}${target != null && data.names ? ` · следующая — ${noteName(target)}` : ""}`,
    ),
    fitted.folded > 0 && h("span", { class: "t-caption" }, `Мелодия шире клавиатуры: ${fitted.folded} нот перенесены на октаву.`),
    h(PianoKeys, {
      from: fitted.from,
      to: fitted.to,
      next: playing != null ? [fitted.notes[playing]?.[0]].filter((n) => n != null) : target != null ? [target] : [],
      down,
      wrong: wrong != null ? [wrong] : [],
      names: data.names,
      onDown: (n) => midi.press(n),
      onUp: (n) => midi.release(n),
    }),
    h(
      "div",
      { class: "flex gap-6" },
      h("button", { class: "pill", type: "button", onClick: restart }, "Сначала"),
      h("button", { class: "pill", type: "button", onClick: listen }, playing != null ? "Стоп" : "Послушать"),
    ),
  );
}

export default definePlugin({
  id: ID,
  sections: [{ id: ID, title: "__TITLE__", icon: icons.music, Screen: Lesson, nav: "link" }],
  slots: {
    "desk.widgets": [{ id: "main", title: "__TITLE__", icon: icons.music, size: "small", initial: true, source: "приложение", Component: Widget }],
  },
});
