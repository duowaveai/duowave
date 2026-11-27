// Окно-подсказка рядом с «Системными настройками» Mac (docs/plans/permissions.md; приложение — desktop/src-tauri/src/
// guide.rs): `#/guide/<item>`. Маленькое окно без рамки у правого края окна настроек: «Найдите DuoWave в списке и
// включите переключатель». Само следит за разрешением: включили — «Готово» и окно закрывается; × — закрыть сразу.

import { useEffect, useState } from 'preact/hooks';

import { type PermissionItem, api } from '../api';
import { tr } from '../i18n';
import '../ui/permissions.css';
import '../ui/guide.css';

/** Сколько «Готово» видно, прежде чем окно закроется. */
const DONE_MS = 1500;

export function GuidePage({ item }: { item: PermissionItem }) {
    const t = tr.permissions;
    const [done, setDone] = useState(false);
    const close = () => void api.permissionGuide(item, false).catch(() => undefined);
    useEffect(() => {
        document.documentElement.classList.add('guide-page');
    }, []);
    useEffect(() => {
        let alive = true;
        const read = () =>
            api
                .permissions()
                .then((view) => {
                    const row = view.items.find((entry) => entry.item === item);
                    if (alive && row?.state === 'granted') setDone(true);
                })
                .catch(() => undefined);
        void read();
        const timer = window.setInterval(() => void read(), 1000);
        return () => {
            alive = false;
            window.clearInterval(timer);
        };
    }, [item]);
    useEffect(() => {
        if (!done) return;
        const timer = window.setTimeout(close, DONE_MS);
        return () => window.clearTimeout(timer);
    }, [done]);
    return (
        <main class={`guide${done ? ' is-done' : ''}`}>
            <span class="guide-arrow" aria-hidden="true">
                <svg viewBox="0 0 24 24">{done ? <path d="M5 12.5l4.5 4.5L19 7.5" /> : <path d="M19 12H5M11 6l-6 6 6 6" />}</svg>
            </span>
            <p role="status">{done ? t.guideDone : t.guide}</p>
            <button type="button" class="guide-x" aria-label={t.guideClose} title={t.guideClose} onClick={close}>
                <svg viewBox="0 0 24 24" aria-hidden="true">
                    <path d="M6 6l12 12M18 6 6 18" />
                </svg>
            </button>
        </main>
    );
}
