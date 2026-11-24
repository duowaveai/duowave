// __TITLE__: виджет на столе и окно урока. Клавиатуру слышит useMidi (шлюз на компьютере; в «Примерке» — клавиатура
// на экране), звук — useSynth, клавиши — PianoKeys (docs/plugins/AUTHORING.md, «MIDI-клавиатура»). Урок «ждёт тебя»:
// темпа нет, следующая нота светится, пока её не нажмут. Прогресс сохраняется одним вызовом в конце, не на каждую
// клавишу. Тексты — словарём по языкам (useT): язык пульта выбирает окошко. Свою мелодию файлом .mid читает шлюз;
// где читать некому (readMidiFile — null), кнопки «Своя мелодия…» нет.
import { h } from "preact";
import { useEffect, useMemo, useRef, useState } from "preact/hooks";
import { PianoKeys, definePlugin, icons, noteName, readMidiFile, melodyOf, t as translate, useApi, useMidi, usePanelState, useSynth, useT } from "@duowave/panel-sdk";

const ID = "__ID__";
const TITLE = "__TITLE__";

const T = {
  ru: {
    source: "приложение",
    reading: "Читаю…",
    noMelodies: "Мелодий нет",
    passed: "Пройдена {times} раз · лучший раз — ошибок {best}",
    notPassed: "Ещё не пройдена",
    open: "Открыть урок",
    onlyDrums: "В файле нет мелодии: только ударные.",
    added: "Мелодия «{title}» добавлена.",
    howMany: "Сколько клавиш у «{name}»?",
    byName: " По названию — {keys}.",
    leftmost: "Нажмите на клавиатуре самую левую клавишу.",
    ownSound: "Клавиатура звучит сама, без компьютера?",
    yesOwn: "Да, у неё свой звук",
    noPanel: "Нет, звук — из пульта",
    own: "Своя мелодия…",
    remove: "Убрать",
    whichTrack: "В файле {count} дорожки — какую учить?",
    track: "Дорожка {n}",
    notes: "{name} · {count} нот",
    viaBrowser: "через браузер",
    viaComputer: "через компьютер",
    delay: " · задержка {ms} мс",
    connecting: "Подключаю клавиатуру…",
    keyboardAgain: "Клавиатура заново",
    connectBrowser: "Подключить через браузер",
    startSound: "Начать — включить звук",
    bluetooth: "Звук идёт {ms} мс — наверное, наушники по Bluetooth. Играть удобнее со звуком клавиатуры или из динамиков.",
    done: "Готово! «{title}», ошибок: {mistakes}. Ещё раз?",
    progress: "«{title}» · {index} из {total}",
    mistakes: " · ошибок {count}",
    next: " · следующая — {note}",
    folded: "Мелодия шире клавиатуры: {count} нот перенесены на октаву.",
    restart: "Сначала",
    stop: "Стоп",
    listen: "Послушать",
  },
  en: {
    source: "app",
    reading: "Reading…",
    noMelodies: "No tunes",
    passed: "Played {times} times · best — {best} mistakes",
    notPassed: "Not played yet",
    open: "Open the lesson",
    onlyDrums: "The file has no tune: drums only.",
    added: "The tune “{title}” is added.",
    howMany: "How many keys does “{name}” have?",
    byName: " By its name — {keys}.",
    leftmost: "Press the leftmost key on your keyboard.",
    ownSound: "Does the keyboard make sound by itself, without the computer?",
    yesOwn: "Yes, it has its own sound",
    noPanel: "No, the panel plays the sound",
    own: "My own tune…",
    remove: "Remove",
    whichTrack: "The file has {count} tracks — which one to learn?",
    track: "Track {n}",
    notes: "{name} · {count} notes",
    viaBrowser: "through the browser",
    viaComputer: "through the computer",
    delay: " · delay {ms} ms",
    connecting: "Connecting the keyboard…",
    keyboardAgain: "Keyboard again",
    connectBrowser: "Connect through the browser",
    startSound: "Start — turn the sound on",
    bluetooth: "The sound comes {ms} ms late — Bluetooth headphones, probably. Play with the keyboard’s own sound or the speakers.",
    done: "Done! “{title}”, mistakes: {mistakes}. Again?",
    progress: "“{title}” · {index} of {total}",
    mistakes: " · {count} mistakes",
    next: " · next — {note}",
    folded: "The tune is wider than the keyboard: {count} notes moved by an octave.",
    restart: "From the start",
    stop: "Stop",
    listen: "Listen",
  },
};
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
  const t = useT(T);
  const { data, error } = useTeacher();
  const panel = usePanelState();
  if (error) return h("p", { class: "w-cap m-0" }, error);
  if (!data) return h("p", { class: "w-cap m-0" }, t("reading"));
  const melody = data.melodies.find((m) => m.id === data.current) ?? data.melodies[0];
  const progress = data.progress[melody?.id];
  const first = melody?.notes[0]?.[0] ?? 60;
  const from = 12 * Math.floor(first / 12);
  return h(
    "div",
    { class: "flex flex-col gap-6" },
    h("span", { class: "w-big" }, melody?.title ?? t("noMelodies")),
    h("span", { class: "w-cap" }, progress ? t("passed", { times: progress.times, best: progress.best }) : t("notPassed")),
    h(PianoKeys, { from, to: from + 12, next: [first], names: false, height: 44 }),
    h("button", { class: "w-btn", type: "button", onClick: () => panel.go(ID) }, t("open")),
  );
}

function Lesson() {
  const t = useT(T);
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
      if (usable.length === 0) throw new Error(t("onlyDrums"));
      if (usable.length === 1) return save(chosen.name, song.tempo, usable[0]);
      setTracks({ name: chosen.name, tempo: song.tempo, list: usable });
    } catch (e) {
      setNote(e?.message ?? String(e));
    }
  };
  const save = (name, tempo, track) => {
    setTracks(null);
    const title = name.replace(/\.midi?$/i, "") + (track.name ? ` · ${track.name}` : "");
    api.call("add", { title, tempo, notes: melodyOf(track) }).then(() => setNote(t("added", { title }))).catch((e) => setNote(e?.message ?? String(e)));
  };

  if (error) return h("p", { class: "t-caption" }, error);
  if (!data) return h("p", { class: "t-caption" }, t("reading"));

  const guess = sizeByName(deviceName);
  const steps =
    setup &&
    h(
      "div",
      { class: "flex flex-col gap-6" },
      setup.step === "keys" && [
        h("span", null, t("howMany", { name: deviceName }) + (guess ? t("byName", { keys: guess }) : "")),
        h("div", { class: "flex gap-6", style: { flexWrap: "wrap" } }, ...SIZES.map((keys) =>
          h("button", { class: keys === guess ? "pill pill-own" : "pill", type: "button", onClick: () => api.call("keyboard", { name: deviceName, keys }).then(() => setSetup({ step: "low" })) }, String(keys)))),
      ],
      setup.step === "low" && h("span", null, t("leftmost")),
      setup.step === "sounds" && [
        h("span", null, t("ownSound")),
        h("div", { class: "flex gap-6" },
          h("button", { class: "pill", type: "button", onClick: () => api.call("keyboard", { name: deviceName, sounds: true }).then(() => setSetup(null)) }, t("yesOwn")),
          h("button", { class: "pill", type: "button", onClick: () => api.call("keyboard", { name: deviceName, sounds: false }).then(() => setSetup(null)) }, t("noPanel"))),
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
      readMidiFile && h("button", { class: "pill", type: "button", onClick: () => file.current?.click() }, t("own")),
      melody?.own && h("button", { class: "pill", type: "button", onClick: () => api.call("remove", { id: melody.id }) }, t("remove")),
      readMidiFile && h("input", { ref: file, type: "file", accept: ".mid,.midi,audio/midi", hidden: true, onChange: pick }),
    ),
    tracks && h("div", { class: "flex flex-col gap-6" },
      h("span", null, t("whichTrack", { count: tracks.list.length })),
      h("div", { class: "flex gap-6", style: { flexWrap: "wrap" } }, ...tracks.list.map((track) =>
        h("button", { class: "pill", type: "button", onClick: () => save(tracks.name, tracks.tempo, track) }, t("notes", { name: track.name ?? t("track", { n: track.index + 1 }), count: track.melody.length }))))),
    note && h("span", { class: "t-caption" }, note),
    h(
      "div",
      { class: "flex gap-6", style: { flexWrap: "wrap", alignItems: "center" } },
      h(
        "span",
        { class: "t-caption" },
        midi.state === "ready"
          ? `${midi.input?.name} · ${midi.via === "browser" ? t("viaBrowser") : t("viaComputer")}${midi.delayMs != null && midi.delayMs > 20 ? t("delay", { ms: midi.delayMs }) : ""}`
          : midi.why ?? t("connecting"),
      ),
      // Ответили не так — три шага заново.
      board?.keys && !setup &&
        h("button", { class: "pill", type: "button", onClick: () => api.call("keyboard", { name: deviceName, reset: true }).then(() => setSetup({ step: "keys" })) }, t("keyboardAgain")),
    ),
    midi.inputs.length > 1 && h("select", { value: midi.input?.id, onChange: (e) => midi.choose(e.currentTarget.value) },
      ...midi.inputs.map((port) => h("option", { value: port.id }, port.name))),
    (midi.state === "browser" || (midi.state === "nodevice" && midi.browserPossible)) &&
      h("button", { class: "pill", type: "button", onClick: () => midi.useBrowser() }, t("connectBrowser")),
    steps,
    !sounds && synth.needsTap && h("button", { class: "pill", type: "button", onClick: () => synth.start() }, t("startSound")),
    synth.delayMs != null && synth.delayMs > 80 && !sounds &&
      h("span", { class: "t-caption" }, t("bluetooth", { ms: synth.delayMs })),
    h(
      "span",
      null,
      finished
        ? t("done", { title: melody.title, mistakes })
        : t("progress", { title: melody?.title, index, total: fitted.notes.length }) +
            (mistakes ? t("mistakes", { count: mistakes }) : "") +
            (target != null && data.names ? t("next", { note: noteName(target) }) : ""),
    ),
    fitted.folded > 0 && h("span", { class: "t-caption" }, t("folded", { count: fitted.folded })),
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
      h("button", { class: "pill", type: "button", onClick: restart }, t("restart")),
      h("button", { class: "pill", type: "button", onClick: listen }, playing != null ? t("stop") : t("listen")),
    ),
  );
}

export default definePlugin({
  id: ID,
  sections: [{ id: ID, title: TITLE, icon: icons.music, Screen: Lesson, nav: "link" }],
  slots: {
    "desk.widgets": [{ id: "main", title: TITLE, icon: icons.music, size: "small", initial: true, source: translate(T, "source"), Component: Widget }],
  },
});
