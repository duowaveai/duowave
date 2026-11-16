import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import { defineConfig, type Plugin } from 'vite';
import preact from '@preact/preset-vite';

import { DEFAULT_THEME, bootAttributes, bootCss } from './src/shell/themes';

// Dev: the panel talks to the gateway through the Vite proxy (same origin, no CORS).
// The gateway listens on PORT (.env), 7780 unless set (a gateway set up before 0.1.1 may say 80): GATEWAY_URL for another.
const gateway = process.env.GATEWAY_URL ?? 'http://127.0.0.1:7780';
const here = dirname(fileURLToPath(import.meta.url));

// Тема по умолчанию вшивается в index.html из тех же данных (src/shell/themes.ts): первая отрисовка — в её цвета ещё
// до загрузки скриптов, как раньше делал glass.css. Её id, режим и материал — атрибутами: их читает загрузчик темы.
const themeBoot: Plugin = {
    name: 'theme-boot',
    transformIndexHtml: (html) => html.replace('<!-- theme-boot -->', `<style id="theme-boot" ${bootAttributes(DEFAULT_THEME)}>\n${bootCss(DEFAULT_THEME)}\n</style>`),
};

// Шрифты — только woff2. @fontsource к каждому начертанию даёт запасной .woff для браузеров без woff2 (старше Chrome 36,
// Firefox 39, Safari 12), а пульт собран под es2022 — Chrome 94, Firefox 93, Safari 16.4 и новее; woff2 читают все они,
// и окно приложения DuoWave тоже (WebKit, WebView2). Запасные только растили программу колонки (~180 КБ): их ссылки
// уходят из @font-face до сборки, и .woff в dist не попадают. Появится .woff всё же — сборка остановится и скажет.
const WOFF_FALLBACK = /,\s*url\([^)]*\.woff['"]?\s*\)\s*format\(\s*['"]?woff['"]?\s*\)/g;
const woff2Only: Plugin = {
    name: 'woff2-only',
    enforce: 'pre',
    transform(code, id) {
        if (!id.split('?')[0].endsWith('.css')) return null;
        const trimmed = code.replace(WOFF_FALLBACK, '');
        return trimmed === code ? null : { code: trimmed, map: null };
    },
    generateBundle(_, bundle) {
        const woff = Object.keys(bundle).filter((name) => name.endsWith('.woff'));
        if (woff.length > 0) this.error(`в сборке пульта .woff, а шрифты — только woff2 (vite.config.ts, woff2Only): ${woff.join(', ')}`);
    },
};

export default defineConfig({
    // Прод-сборка живёт на гейтвее под /panel — сборка ссылается на свои ассеты оттуда же.
    base: '/panel/',
    plugins: [preact(), themeBoot, woff2Only],
    server: {
        proxy: {
            '/health': { target: gateway },
            // ws: живой канал — WebSocket (/api/v1/live/ws).
            '/api': { target: gateway, ws: true },
            '/debug': { target: gateway },
            // Части плагинов и проход помощника к колонке (docs/plans/desk.md, этап 4).
            '/panel-plugins': { target: gateway },
            '/panel-preview': { target: gateway },
            '/station': { target: gateway },
        },
    },
    // Проверка прод-сборки: pnpm --filter panel preview
    preview: {
        proxy: {
            '/health': { target: gateway },
            '/api': { target: gateway, ws: true },
            '/debug': { target: gateway },
            '/panel-plugins': { target: gateway },
            '/panel-preview': { target: gateway },
            '/station': { target: gateway },
        },
    },
    // Две страницы: пульт и окошко приложения (frame.html — изолированный фрейм для кода приложений, docs/plans/bus.md).
    build: {
        outDir: 'dist',
        target: 'es2022',
        rollupOptions: { input: { main: resolve(here, 'index.html'), frame: resolve(here, 'frame.html') } },
    },
});
