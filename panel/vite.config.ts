import { defineConfig, type Plugin } from 'vite';
import preact from '@preact/preset-vite';

import { DEFAULT_THEME, bootAttributes, bootCss } from './src/shell/themes';

// Dev: the panel talks to the gateway through the Vite proxy (same origin, no CORS).
// The gateway listens on PORT (.env), 7780 unless set (a gateway set up before 0.1.1 may say 80): GATEWAY_URL for another.
const gateway = process.env.GATEWAY_URL ?? 'http://127.0.0.1:7780';

// Тема по умолчанию вшивается в index.html из тех же данных (src/shell/themes.ts): первая отрисовка — в её цвета ещё
// до загрузки скриптов, как раньше делал glass.css. Её id, режим и материал — атрибутами: их читает загрузчик темы.
const themeBoot: Plugin = {
    name: 'theme-boot',
    transformIndexHtml: (html) => html.replace('<!-- theme-boot -->', `<style id="theme-boot" ${bootAttributes(DEFAULT_THEME)}>\n${bootCss(DEFAULT_THEME)}\n</style>`),
};

export default defineConfig({
    // Прод-сборка живёт на гейтвее под /panel — сборка ссылается на свои ассеты оттуда же.
    base: '/panel/',
    plugins: [preact(), themeBoot],
    server: {
        proxy: {
            '/health': { target: gateway },
            '/api': { target: gateway },
            '/debug': { target: gateway },
        },
    },
    // Проверка прод-сборки: pnpm --filter panel preview
    preview: {
        proxy: {
            '/health': { target: gateway },
            '/api': { target: gateway },
            '/debug': { target: gateway },
        },
    },
    build: { outDir: 'dist', target: 'es2022' },
});
