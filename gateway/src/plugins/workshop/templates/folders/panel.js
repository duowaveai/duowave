// __TITLE__: окошко со всеми фото папки «Фото» сеткой. Картинку даёт шлюз — useFolderFile(адрес): уменьшенной и по
// связи приложения с папкой; сам код фото не читает. Нажатие — фото крупно, ещё раз — обратно к сетке.
import { h } from "preact";
import { useEffect, useState } from "preact/hooks";
import { definePlugin, icons, useApi, useFolderFile, usePanelState } from "@duowave/panel-sdk";

const ID = "__ID__";

function Thumb({ photo, big, onPick }) {
  const url = useFolderFile(photo.address, { size: big ? 1600 : 320 });
  return h(
    "button",
    {
      onClick: onPick,
      title: photo.name,
      style: {
        border: 0,
        padding: 0,
        borderRadius: "12px",
        overflow: "hidden",
        background: "var(--pill-off-bg)",
        aspectRatio: big ? "auto" : "4 / 3",
        cursor: "pointer",
      },
    },
    url ? h("img", { src: url, alt: photo.name, style: { width: "100%", height: "100%", objectFit: big ? "contain" : "cover", display: "block" } }) : null,
  );
}

function Album() {
  const api = useApi(ID);
  const { version } = usePanelState();
  const [shown, setShown] = useState({ total: 0, items: [] });
  const [big, setBig] = useState(null);
  const [error, setError] = useState(null);
  useEffect(() => {
    api.call("page", { from: 0 }).then(setShown).catch((e) => setError(e?.message ?? String(e)));
  }, [api, version]);
  const more = () => api.call("page", { from: shown.items.length }).then((next) => setShown({ total: next.total, items: [...shown.items, ...next.items] }));
  if (error) return h("p", { class: "t-caption" }, error);
  if (big) return h(Thumb, { photo: big, big: true, onPick: () => setBig(null) });
  return h(
    "div",
    { style: { display: "flex", flexDirection: "column", gap: "12px" } },
    h("p", { class: "t-caption", style: { margin: 0 } }, shown.total ? `Фото: ${shown.total}` : "В папке «Фото» пока пусто."),
    h(
      "div",
      { style: { display: "grid", gridTemplateColumns: "repeat(auto-fill, minmax(140px, 1fr))", gap: "10px" } },
      shown.items.map((photo) => h(Thumb, { key: photo.address, photo, onPick: () => setBig(photo) })),
    ),
    shown.items.length < shown.total ? h("button", { class: "pill pill-off", onClick: more }, "Ещё") : null,
  );
}

export default definePlugin({
  id: ID,
  sections: [{ id: ID, title: "__TITLE__", icon: icons.sparkle, Screen: Album, nav: "link" }],
});
