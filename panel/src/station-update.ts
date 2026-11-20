// Что можно сделать с версией шлюза колонки — так, как это видит компьютер, который её ведёт (owner 20.11: колонка
// писала «Доступна 0.16.3», а «Обновить» отвечало «У колонки последняя версия»). Компьютер ставит колонке только сборку
// своей версии: выпуск новее компьютера идёт сначала на компьютер, колонка — следом (gateway/src/placement/watch.rs,
// `plan_update`). Чистая логика без DOM: её проверяет panel/scripts/panel-check.ts.

import type { StationGatewayView } from './api';

/** Шаг для людей: кнопка и что она сделает — или почему кнопки нет. */
export type StationUpdateStep =
    /** «Обновить колонку до X»: компьютер отправит ей свою сборку сейчас. */
    | { kind: 'send'; version: string }
    /** «Обновить компьютер и колонку до X»: сначала обновится компьютер, колонка — следом, сама. */
    | { kind: 'computerFirst'; version: string }
    /** Колонке можно новее, но отсюда не поставить: почему и что сделать. */
    | { kind: 'blocked'; version: string; why: string }
    /** На колонке версия новее компьютера: обновлять надо компьютер. */
    | { kind: 'stationAhead'; station: string; computer: string }
    /** Компьютер обновляется, колонка обновится следом — нажимать нечего. */
    | { kind: 'follows'; version: string }
    /** Колонка сама видит версию новее, а компьютер о ней ещё не сказал: кнопка спросит его, и он решит сам. */
    | { kind: 'ask'; version: string };

/**
 * Шаг по виду компьютера на колонку (`view`) и версии, которую колонка увидела в ленте сама (`seen`, только в окне
 * колонки). Null — обновлять нечего, «последняя версия» — правда.
 */
export function stationUpdateStep(
    view: Pick<StationGatewayView, 'plan' | 'afterComputer'> | null | undefined,
    seen?: string | null,
): StationUpdateStep | null {
    if (view?.afterComputer) return { kind: 'follows', version: view.afterComputer };
    const plan = view?.plan;
    if (plan) {
        switch (plan.way) {
            case 'send':
                return { kind: 'send', version: plan.version };
            case 'computerFirst':
                return { kind: 'computerFirst', version: plan.version };
            case 'blocked':
                return { kind: 'blocked', version: plan.version, why: plan.why };
            case 'stationAhead':
                return { kind: 'stationAhead', station: plan.station, computer: plan.computer };
        }
    }
    return seen ? { kind: 'ask', version: seen } : null;
}

/** Версия, которую стоит поставить колонке (точка у «Настроек»), — когда для неё есть что-то новее. */
export function stationOfferOf(step: StationUpdateStep | null): string | null {
    if (!step) return null;
    switch (step.kind) {
        case 'send':
        case 'computerFirst':
        case 'blocked':
        case 'ask':
            return step.version;
        default:
            return null;
    }
}

/** Есть ли кнопка: шаг, который делает компьютер по нажатию. */
export function stationUpdatePressable(step: StationUpdateStep | null): step is Extract<StationUpdateStep, { kind: 'send' | 'computerFirst' | 'ask' }> {
    return !!step && (step.kind === 'send' || step.kind === 'computerFirst' || step.kind === 'ask');
}
