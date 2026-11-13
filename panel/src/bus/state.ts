// Перемены шины для пульта (docs/plans/bus.md): живой канал говорит `bus.changed` (связи, режимы, ответы), `bus.asked`
// (приложение ждёт «да») и `bus.news` (приложение рассказало новость). Экраны «Связей» и лист «да» перечитывают своё по
// своему номеру, а не по каждой перемене в пульте.

import { useEffect, useState } from 'preact/hooks';

let version = 0;
const listeners = new Set<() => void>();

/** Перемена, о которой сказал живой канал: если она о шине — тем, кто на экране, перечитать. */
export function noteBusChange(name: string): void {
    if (name !== 'hello' && name !== 'bus.changed' && name !== 'bus.asked') return;
    version += 1;
    for (const listener of [...listeners]) listener();
}

/** Сказать о перемене самому — после своего ответа или правки, не дожидаясь канала. */
export function busChanged(): void {
    noteBusChange('bus.changed');
}

/** Номер перемены шины: компонент перерисуется, когда он вырастет. */
export function useBusVersion(): number {
    const [seen, setSeen] = useState(version);
    useEffect(() => {
        const update = (): void => setSeen(version);
        listeners.add(update);
        update();
        return () => {
            listeners.delete(update);
        };
    }, []);
    return seen;
}
