// Плашка «Делаю виджет» без DOM (shell/agent-plaque.tsx): видна ли закончившаяся работа. Проверяет panel-check.
//
// Закончившая работа видна полторы минуты — «Готово», «Не вышло», «Остановлено». Но если вещь за это время уже
// поставили, убрали в черновики или удалили, «Готово · «NOIR / CUT»» справа сверху — неправда о том, что ещё ждёт
// человека (владелец, 19.11: тему применил, а плашка осталась). Решённое уходит с плашки сразу.

/** Перемены мастерской, после которых о закончившейся работе над черновиком решено. */
export const DECIDED_CHANGES = ['installed', 'parked', 'deleted'];

/**
 * Видна ли на плашке закончившаяся работа: о ней не решили после её конца. `ended` — конец по часам её шлюза,
 * `offset` — его часы минус наши, `decidedAt` — когда решили, по нашим часам.
 */
export function endedStillShown(ended: number | undefined, offset: number, decidedAt: number | undefined): boolean {
    if (decidedAt === undefined || ended === undefined) return true;
    return decidedAt < ended - offset;
}
