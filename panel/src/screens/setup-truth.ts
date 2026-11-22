// Что ещё не готово сразу после выбора (замечание 17 после 0.18.1): владелец быстро прошёл первые шаги на колонке, выбрал
// «Джарвиса» и не выбрал мышление — пульт говорил «скажите «Джарвис»», а персонаж ещё ставился, слово в режиме «Алиса»
// не слышно (его ловит только «Алиса + ваш ИИ»), голоса Джарвиса без компьютера-Турбо нет — отвечала Алиса. Здесь —
// то, что пульт уже знает: ход установки (мастер, magicplace/wizard-state.ts), слово (call-word.ts), режим колонки,
// выбранный персонаж и его голос, Турбо с голосом в доме (/health). Шаг «Попробуйте сказать» и витрина стола говорят это
// честно и, пока что-то не готово, зовут прежним словом.

import { useEffect, useState } from 'preact/hooks';

import { CharactersView, ModeView, PanelMode, api, stationCall } from '../api';
import { InstallProgress, installProgress, onInstallProgress } from '../magicplace/wizard-state';
import { usePanel } from '../shell/context';
import { plainName } from '../voice/pending-voice';
import { useCallWord } from './call-word';

export interface SetupTruth {
    /** Установка, что идёт сейчас (персонаж, слово, голос): название и ход; null — ничего не ставится. */
    installing: InstallProgress | null;
    /** Слово приложения-слова («Джарвис»); null — его нет. */
    word: string | null;
    /** Режим колонки; undefined — ещё не прочитан или колонки нет. */
    mode: PanelMode | undefined;
    /** Выбранный персонаж не из встроенных: его имя без «(демо)». */
    character: string | null;
    /** Голос персонажа здесь не звучит, а Турбо с голосом в доме нет: отвечает голос Алисы. */
    voiceAway: boolean;
}

/** То, что не готово: читается при открытии и по новостям шлюза. */
export function useSetupTruth(): SetupTruth {
    const panel = usePanel();
    const word = useCallWord();
    const [installing, setInstalling] = useState(installProgress);
    useEffect(() => onInstallProgress(() => setInstalling(installProgress())), []);
    const [mode, setMode] = useState<PanelMode | undefined>(undefined);
    const [characters, setCharacters] = useState<CharactersView | null>(null);
    const helperOf = panel.role.role === 'helper' ? panel.role.station?.key : undefined;
    const onStation = panel.health?.placement?.role === 'station';
    useEffect(() => {
        if (!onStation && !helperOf) return;
        let alive = true;
        void (helperOf ? stationCall<ModeView>(helperOf, 'GET', '/api/v1/station/mode') : api.mode())
            .then((view) => alive && setMode(view.mode))
            .catch(() => undefined);
        return () => {
            alive = false;
        };
    }, [onStation, helperOf, panel.version]);
    useEffect(() => {
        let alive = true;
        void api
            .characters()
            .then((view) => alive && setCharacters(view))
            .catch(() => undefined);
        return () => {
            alive = false;
        };
    }, [panel.version, installing?.finished]);
    const id = characters?.active?.id;
    const active = id ? characters?.characters.find((entry) => entry.id === id) : undefined;
    const character = active && !active.builtin ? plainName(active.title) : null;
    // Голос персонажа на колонке — с компьютера-Турбо: его модели там. Нет Турбо, что даёт голос, — говорит Алиса.
    const turboVoice = (panel.health?.placement?.helpers?.list ?? []).some((helper) => helper.state === 'up' && helper.abilities.includes('voice'));
    const voiceAway = onStation && !!active && !active.builtin && (active.voice.state === 'fallback' || active.voice.state === 'unsupported') && !turboVoice;
    return { installing: installing && !installing.finished ? installing : null, word, mode, character, voiceAway };
}
