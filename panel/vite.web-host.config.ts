import { readFileSync, readdirSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import { defineConfig, type Plugin } from 'vite';
import preact from '@preact/preset-vite';

// Веб-хозяин (docs/plans/web-host.md): страница хозяина и окошко приложения как статические файлы с относительными
// адресами — их кладут на любой сайт рядом с папкой приложений (../apps/). Собирает scripts/web-tools.ts:
//
//     node panel/scripts/web-tools.ts <папка витрины> <папка пакета>…

const here = dirname(fileURLToPath(import.meta.url));

// Окошку нужны прокладки SDK (public/sdk/*): import map рамки ведёт к ним. Остального из public/ хозяину не надо.
const sdkShims: Plugin = {
    name: 'web-host-sdk',
    generateBundle() {
        const folder = resolve(here, 'public/sdk');
        for (const name of readdirSync(folder)) {
            this.emitFile({ type: 'asset', fileName: `sdk/${name}`, source: readFileSync(resolve(folder, name)) });
        }
    },
};

// Шрифты — только woff2, как у пульта (vite.config.ts).
const WOFF_FALLBACK = /,\s*url\([^)]*\.woff['"]?\s*\)\s*format\(\s*['"]?woff['"]?\s*\)/g;
const woff2Only: Plugin = {
    name: 'woff2-only',
    enforce: 'pre',
    transform(code, id) {
        if (!id.split('?')[0].endsWith('.css')) return null;
        const trimmed = code.replace(WOFF_FALLBACK, '');
        return trimmed === code ? null : { code: trimmed, map: null };
    },
};

export default defineConfig({
    root: resolve(here, 'web-host'),
    base: './',
    publicDir: false,
    plugins: [preact(), sdkShims, woff2Only],
    worker: { format: 'es', rollupOptions: { output: { entryFileNames: 'assets/worker-[hash].js' } } },
    build: {
        outDir: process.env.WEB_HOST_OUT ?? resolve(here, 'dist-web-host'),
        emptyOutDir: true,
        target: 'es2022',
        rollupOptions: { input: { index: resolve(here, 'web-host/index.html'), frame: resolve(here, 'web-host/frame.html') } },
    },
});
