import { Fragment, render } from 'preact';

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
import { ScreenPage } from './screen/page';
import { initLanguage, useLanguage } from './i18n';

// Встроенное — тем же путём, что плагины: разделы, слоты и темы приходят в реестр, тема применяется до первой отрисовки.
registerPlugin(core);
applyTheme();

// Окно на экране компьютера (screen/page.tsx): своё окно приложения DuoWave — только то, что оно показывает.
const screen = /^#\/screen\/([^/?]+)/.exec(location.hash);

/**
 * Корень пульта по языку (docs/plans/i18n.md): смена языка пересоздаёт его — каждый текст читается из словаря нового
 * языка, страница не перезагружается.
 */
function Root() {
    const language = useLanguage();
    return screen ? (
        <ScreenPage key={language} id={decodeURIComponent(screen[1])} />
    ) : (
        <Fragment key={language}>
            <App />
            <ConfirmHost />
        </Fragment>
    );
}

// Язык — до первой отрисовки: выбор на этом устройстве, язык дома со шлюза или язык системы.
void initLanguage().finally(() => render(<Root />, document.getElementById('root')!));
