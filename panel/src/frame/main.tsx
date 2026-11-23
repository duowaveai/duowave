// Страница окошка приложения в пульте шлюза (frame.html; docs/plans/bus.md, «Окошки»): само окошко — boot.tsx, здесь —
// откуда пульту можно брать модуль. Правило страницы — `sandbox allow-scripts`, `connect-src 'none'`
// (gateway/src/panel/controller.rs).

import { bootFrame } from './boot';

/** Модуль только со своего шлюза: части приложений и черновиков, напрямую или через проход помощника. */
function allowed(module: string): string | null {
    try {
        const url = new URL(module, location.href);
        if (url.origin !== location.origin) return null;
        return /^(\/station\/[^/]+)?\/(panel-plugins|panel-preview)\//.test(url.pathname) ? url.href : null;
    } catch {
        return null;
    }
}

bootFrame({ allowed });
