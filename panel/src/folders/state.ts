// Перемены домашней папки для пульта (docs/plans/shared-folders.md): живой канал говорит `folders.changed` — что-то
// подключили или убрали, флешку воткнули или вынули, поменяли галочки. Раздел «Папки» и плитка на столе перечитывают
// своё по своему номеру, а не по каждой перемене в пульте.

import { useEffect, useState } from 'preact/hooks';

let version = 0;
const listeners = new Set<() => void>();

/** Перемена, о которой сказал живой канал: если она о папках — тем, кто на экране, перечитать. */
export function noteFoldersChange(name: string): void {
    if (name !== 'hello' && name !== 'folders.changed' && name !== 'bus.changed') return;
    version += 1;
    for (const listener of [...listeners]) listener();
}

/** Сказать о перемене самому — после своего нажатия, не дожидаясь канала. */
export function foldersChanged(): void {
    noteFoldersChange('folders.changed');
}

/** Номер перемены папок: компонент перерисуется, когда он вырастет. */
export function useFoldersVersion(): number {
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
