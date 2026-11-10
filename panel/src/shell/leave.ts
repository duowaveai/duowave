// Уйти с несохранённым: экран, в котором есть несохранённое (файл в мастерской), спрашивает, прежде чем пульт уйдёт
// с него — окно закроют (× и Esc), выберут другой раздел в доке или перейдут куда-то ещё (app.tsx, go). Ответ
// «остаться» — окно и правка остаются как были. Окно колонки у помощника — пульт колонки во фрейме, со своими
// вопросами: страница помощника спрашивает его (frameMayLeave), а он — своё несохранённое (exposeLeave).

import { useEffect, useRef } from 'preact/hooks';

type Ask = () => Promise<boolean>;

declare global {
    interface Window {
        /** Пульт колонки во фрейме окна помощника (via.ts): можно ли уйти — спросит сам, если есть несохранённое. */
        duowaveMayLeave?: Ask;
    }
}

const guards = new Set<{ current: Ask }>();

/** Пока `active`, уход сначала спрашивает `ask`: true — уйти, false — остаться. */
export function useLeaveGuard(active: boolean, ask: Ask): void {
    const ref = useRef(ask);
    ref.current = ask;
    useEffect(() => {
        if (!active) return;
        guards.add(ref);
        return () => {
            guards.delete(ref);
        };
    }, [active]);
}

/** Есть ли кому спрашивать: без несохранённого переход идёт сразу, не дожидаясь ответа. */
export function leaving(): boolean {
    return guards.size > 0;
}

/** Можно ли уйти: каждый, у кого есть несохранённое, согласен (спрашивает по очереди). */
export async function mayLeave(): Promise<boolean> {
    for (const guard of [...guards]) if (!(await guard.current())) return false;
    return true;
}

/**
 * Пульт колонки во фрейме отдаёт странице помощника свой вопрос: её ×, док и Esc спрашивают о несохранённом здесь
 * (мастерская колонки), прежде чем закрыть окно. Возвращает, как забрать его обратно.
 */
export function exposeLeave(): () => void {
    const ask: Ask = () => (leaving() ? mayLeave() : Promise.resolve(true));
    window.duowaveMayLeave = ask;
    return () => {
        if (window.duowaveMayLeave === ask) delete window.duowaveMayLeave;
    };
}

/** Спросить пульт колонки во фрейме, можно ли уйти. Не ответил (ещё грузится, другой адрес, ошибка) — можно. */
export async function frameMayLeave(frame: HTMLIFrameElement | null): Promise<boolean> {
    try {
        const ask = frame?.contentWindow?.duowaveMayLeave;
        return typeof ask === 'function' ? await ask() : true;
    } catch {
        return true;
    }
}
