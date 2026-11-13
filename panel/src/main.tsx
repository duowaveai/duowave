import { render } from 'preact';

// «Минимал»: заголовки — Inter Tight 300, код — JetBrains Mono 400, текст — Inter 400/500 (ui/fonts-minimal.css).
// Файлы шрифтов в сборке пульта: он работает дома и без интернета.
import '@fontsource/inter-tight/300.css';
import '@fontsource/jetbrains-mono/400.css';
import './ui/fonts-minimal.css';

import './ui/glass.css';
import './ui/packages.css';
import './ui/duowave.css';
import { App } from './app';
// После стилей экранов: классы вместо инлайн-стилей перекрывают их, как перекрывал style="…".
import './ui/utilities.css';
// Материал «Минимал»: его правила под html[data-look='minimal'] перекрывают базовые элементы (ui/glass.css).
import './ui/minimal.css';
// Стол, док, окна и виджеты (docs/plans/desk.md) — последними.
import './ui/desk.css';
import './ui/activity.css';
import './ui/recall.css';
// Клавиатура на экране для приложений (sdk/piano-keys.tsx).
import './ui/piano-keys.css';
import { registerPlugin } from './shell/registry';
import { applyTheme } from './shell/theme';
import core from './builtin';
import { ConfirmHost } from './ui/confirm';

// Встроенное — тем же путём, что плагины: разделы, слоты и темы приходят в реестр, тема применяется до первой отрисовки.
registerPlugin(core);
applyTheme();

render(
    <>
        <App />
        <ConfirmHost />
    </>,
    document.getElementById('root')!,
);
