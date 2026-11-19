// Слова-активаторы дома одним списком (замечание 11 теста 19.11): какие стоят, какое включено, выбрать другое, «Только
// Алиса», убрать все. Слово — приложение с правом `call:system/activator` (magicplace/filters.ts); стоит оно у центра:
// у компьютера-помощника — у колонки (через проход), иначе у своего шлюза. Включено всегда одно: выбрать слово —
// включить его и выключить остальные; на Лайт 2 каждое включённое — ещё 8–14 МБ ОЗУ (docs/plans/wake-word-station.md).
// Выключенное останавливается сразу (ОЗУ свободно), файлы остаются — включается быстро; «Убрать» стирает и файлы.

import { useEffect, useState } from 'preact/hooks';

import { PackageView, installApi } from '../api';
import { ACTIVATOR } from '../magicplace/filters';
import { usePanel } from '../shell/context';

export interface CallWord {
    /** Пакет слова. */
    id: string;
    /** Само слово: «Джарвис». */
    word: string;
    enabled: boolean;
}

/** Слово приложения по названию «Слово «Джарвис» (демо)»; не слово — null. Включено ли — не важно. */
export function callWordOf(pkg: Pick<PackageView, 'id' | 'title' | 'permissions' | 'enabled'>): CallWord | null {
    if (!(pkg.permissions ?? []).includes(ACTIVATOR)) return null;
    return { id: pkg.id, word: /«([^»]+)»/.exec(pkg.title)?.[1] ?? pkg.title, enabled: pkg.enabled };
}

/** Где стоят слова: ключ колонки у компьютера-помощника, иначе свой шлюз. */
export function useWordsHome(): string | undefined {
    const panel = usePanel();
    return panel.role.role === 'helper' ? panel.role.station?.key : undefined;
}

/** Слова дома и перечитать их; null — ещё читаем. */
export function useCallWords(): { words: CallWord[] | null; reload: () => void; station?: string } {
    const panel = usePanel();
    const station = useWordsHome();
    const [words, setWords] = useState<CallWord[] | null>(null);
    const [turn, setTurn] = useState(0);
    useEffect(() => {
        let alive = true;
        void installApi(station)
            .packages()
            .then((view) => alive && setWords(view.packages.map(callWordOf).filter((word): word is CallWord => word !== null)))
            .catch(() => alive && setWords([]));
        return () => {
            alive = false;
        };
    }, [station, panel.version, turn]);
    return { words, reload: () => setTurn((n) => n + 1), station };
}

/** Включить это слово, остальные — выключить: слушает одно. */
export async function chooseWord(station: string | undefined, words: CallWord[], id: string): Promise<void> {
    const api = installApi(station);
    // Сначала выключить прочие — освободить ОЗУ, потом включить выбранное: на Лайт 2 двоим может не хватить.
    for (const other of words.filter((word) => word.id !== id && word.enabled)) await api.disablePackage(other.id);
    if (!words.find((word) => word.id === id)?.enabled) await api.enablePackage(id);
}

/** «Только Алиса»: все слова выключены, колонка откликается на «Алиса». */
export async function aliceOnly(station: string | undefined, words: CallWord[]): Promise<void> {
    const api = installApi(station);
    for (const word of words.filter((word) => word.enabled)) await api.disablePackage(word.id);
}

/** «Убрать все слова»: стереть их с колонки — и ОЗУ, и память (`/data`). */
export async function removeWords(station: string | undefined, words: CallWord[]): Promise<void> {
    const api = installApi(station);
    for (const word of words) await api.removePackage(word.id);
}
