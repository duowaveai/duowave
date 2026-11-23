// Окошко приложения на витрине сайта (web-host/frame.html): то же окошко, что в пульте (frame/boot.tsx). Модуль — только
// из соседней папки витрины ../apps/<id>/, и в SDK нет `readMidiFile`: файлы .mid читает шлюз, а его здесь нет.

import { bootFrame } from '../frame/boot';

const APPS = new URL('../apps/', location.href).href;

function allowed(module: string): string | null {
    try {
        const url = new URL(module, location.href);
        if (url.origin !== location.origin || url.search || url.hash) return null;
        return url.href.startsWith(APPS) && /^[a-z][a-z0-9-]{0,31}\/panel\.(js|css)$/.test(url.href.slice(APPS.length)) ? url.href : null;
    } catch {
        return null;
    }
}

bootFrame({ allowed, sdk: { readMidiFile: null } });
