// «Ждёт вас» без DOM (voice/waiting.tsx): что показывает нажатие на пилюлю. Проверяет panel-check.
//
// Пилюля считает работы, чей вопрос ждёт человека (шлюз, `GET /api/v1/works?waiting=1`), и нажатие обязано показать
// одну из них — 19.11 было «Ждёт вас · 1», а нажатие не делало ничего: полоса умеет только «Готово» с черновиком, и
// работа без строки итога, с вопросом «Доделать?» или с черновиком, которого уже нет, молча пропадала. Теперь так:
// «Готово» и итог фонового дела — на полосе, как было; всё остальное (и то, что полоса не смогла показать) — страница
// работы с её кнопками ответа, а без права на «Работы» — черновик в мастерской.

import type { WorkView } from '../api';

/** Где показать ждущую работу: на полосе — `strip`, иначе — страница. */
export function waitShownOn(work: Pick<WorkView, 'kind' | 'target' | 'result' | 'question'>): 'strip' | 'page' {
    if (!work.result?.line) return 'page';
    if (work.kind === 'tool' || work.kind === 'task') return 'strip';
    return work.question?.kind === 'ready' && !!work.target ? 'strip' : 'page';
}

/** Страница ждущей работы: в «Работах» у того, кто их видит (администратор), иначе — её черновик в мастерской. */
export function waitPage(work: Pick<WorkView, 'id' | 'target'>, works: boolean): { section: string; tab: string; item?: string } | null {
    if (works) return { section: 'works', tab: work.id };
    return work.target ? { section: 'skills', tab: 'workshop', item: work.target } : null;
}
