import { useEffect, useState } from 'preact/hooks';

import { Sheet } from './sheet';
import { tr } from '../i18n';

/**
 * Подтверждение в самом пульте вместо window.confirm: в окне приложения (WebKit внутри Tauri) системное окно не
 * показывается, confirm молча отвечает «нет» — и «обновить», «удалить», «выйти» не делали ничего. Здесь тот же шит,
 * что у остальных окон пульта, в любой теме, в браузере и в приложении.
 *
 *     if (!(await ask({ title: 'Удалить голос?', text: 'Вернуть — только скачав заново.', ok: 'Удалить', danger: true }))) return;
 *
 * Отвечает true по кнопке подтверждения, false — по «Отмена», Esc и нажатию мимо. Окна идут по одному: второй
 * вопрос ждёт ответа на первый. ConfirmHost стоит в корне пульта (main.tsx); без него — window.confirm, как раньше.
 */
export interface AskOptions {
    title: string;
    /** Что будет и можно ли вернуть: строкой под заголовком. */
    text?: string;
    /** Кнопка согласия: глагол действия («Удалить», «Поставить»), не «ОК». */
    ok?: string;
    cancel?: string;
    /** Удаляет или выключает что-то насовсем: кнопка красная. */
    danger?: boolean;
}

interface Pending extends AskOptions {
    answer: (yes: boolean) => void;
}

let show: ((next: Pending | null) => void) | null = null;
const queue: Pending[] = [];

function next(): void {
    show?.(queue[0] ?? null);
}

export function ask(options: AskOptions | string): Promise<boolean> {
    const asked: AskOptions = typeof options === 'string' ? { title: options } : options;
    if (!show) {
        const line = asked.text ? `${asked.title}\n\n${asked.text}` : asked.title;
        return Promise.resolve(window.confirm(line));
    }
    return new Promise((resolve) => {
        queue.push({
            ...asked,
            answer: (yes) => {
                queue.shift();
                resolve(yes);
                next();
            },
        });
        if (queue.length === 1) next();
    });
}

/** Место, где показываются вопросы ask(): один на пульт, в корне. */
export function ConfirmHost() {
    const [current, setCurrent] = useState<Pending | null>(null);
    useEffect(() => {
        show = setCurrent;
        next();
        return () => {
            show = null;
        };
    }, []);
    if (!current) return null;
    // Слой вопросов — над всем: над «+», окном раздела на телефоне и окном, из которого спросили. Маленькое окно
    // посередине: вопрос, что будет, две кнопки справа; опасная — красная, «Отмена» рядом и в фокусе.
    return (
        <Sheet
            key={queue.length + current.title}
            layer="confirm"
            shape="dialog"
            title={current.title}
            onClose={() => current.answer(false)}
            footer={
                <>
                    {/* Удаление не случается по Enter: фокус сначала на «Отмена». */}
                    <button class="btn btn-plain" data-autofocus={current.danger ? true : undefined} onClick={() => current.answer(false)}>
                        {current.cancel ?? tr.ui.cancel}
                    </button>
                    <button class={`btn ${current.danger ? 'btn-danger-fill' : 'btn-main'}`} onClick={() => current.answer(true)}>
                        {current.ok ?? tr.ui.yes}
                    </button>
                </>
            }
        >
            {current.text && <p class="win-text m-0 pre-line max-h-52vh overflow-auto">{current.text}</p>}
        </Sheet>
    );
}
