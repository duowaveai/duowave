// Запросы в дом по коду (docs/plans/home-setup-flow.md, этап 2): центр показывает администратору, кто просится в дом,
// с кодом — «Разрешить» или «Это не я». Список читается по новостям живого канала (`home.join`, app.tsx) и раз в 15 с,
// пока есть открытый запрос. Код видят только администраторы: остальным шлюз список не отдаёт, и пульт его не просит.

import { useEffect, useState } from 'preact/hooks';

import { HomeJoinRequest, api, can } from './api';

let cache: HomeJoinRequest[] = [];
let reading: Promise<void> | null = null;
const listeners = new Set<() => void>();

const tell = (): void => listeners.forEach((listener) => listener());

function read(): Promise<void> {
    if (!can('users.manage')) return Promise.resolve();
    if (reading) return reading;
    reading = api
        .homeJoins()
        .then((list) => {
            cache = list;
            tell();
        })
        // Шлюз постарше без кодов, нет права — карточек просто нет.
        .catch(() => undefined)
        .finally(() => {
            reading = null;
        });
    return reading;
}

/** Новость живого канала: запрос появился, вошёл, закрылся — список читается снова. */
export function noteHomeJoinNews(name: string): void {
    if (name === 'home.join' || (name === 'hello' && listeners.size > 0)) void read();
}

/** Открытые запросы в дом: новые сверху. */
export function useHomeJoins(): HomeJoinRequest[] {
    const [, rerender] = useState(0);
    useEffect(() => {
        const update = (): void => rerender((n) => n + 1);
        listeners.add(update);
        void read();
        const timer = window.setInterval(() => {
            if (cache.some((request) => request.status === 'pending')) void read();
        }, 15_000);
        return () => {
            listeners.delete(update);
            window.clearInterval(timer);
        };
    }, []);
    const now = Date.now();
    return cache.filter((request) => request.status === 'pending' && Date.parse(request.expiresAt) > now);
}

/** Ответ администратора — и список сразу заново. */
export async function answerJoin(id: string, allow: boolean): Promise<void> {
    await (allow ? api.approveJoin(id) : api.denyJoin(id));
    await read();
}

/** Код глазами: «47 19 82». */
export function codePairs(code: string): string {
    return code.replace(/\D/g, '').replace(/(\d{2})(?=\d)/g, '$1 ');
}
