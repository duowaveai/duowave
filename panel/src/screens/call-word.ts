// Слово, которым персонажа зовут на деле (проверка начисто 18.11, п. 54): примеры «Алиса, как дела?» звали Алису, а
// отвечал выбранный персонаж, и стояло слово «Джарвис». Источник тот же, что у мастера установки (п. 49, «звать как
// прежде: …»): включённое приложение-слово (право `call:system/activator`), его слово — из названия «Слово «Джарвис»».
// Нет такого — null, и примеры говорят прежнее слово («Алиса» у колонки, «ДиВи» у компьютера).

import { useEffect, useState } from 'preact/hooks';

import { PackageView, installApi } from '../api';
import { ACTIVATOR } from '../magicplace/filters';
import { usePanel } from '../shell/context';

/** Слово приложения-слова по его названию: «Слово «Джарвис» (демо)» → «Джарвис». */
export function wordOfPackage(pkg: Pick<PackageView, 'title' | 'permissions' | 'enabled'>): string | null {
    if (!pkg.enabled || !(pkg.permissions ?? []).includes(ACTIVATOR)) return null;
    return /«([^»]+)»/.exec(pkg.title)?.[1] ?? null;
}

/** Слово дома сейчас: у компьютера-помощника — у центра (там стоит слово), иначе у своего шлюза. */
export function useCallWord(): string | null {
    const panel = usePanel();
    const station = panel.role.role === 'helper' ? panel.role.station?.key : undefined;
    const [word, setWord] = useState<string | null>(null);
    useEffect(() => {
        let alive = true;
        void installApi(station)
            .packages()
            .then((view) => alive && setWord(view.packages.map(wordOfPackage).find(Boolean) ?? null))
            .catch(() => undefined);
        return () => {
            alive = false;
        };
    }, [station, panel.version]);
    return word;
}
