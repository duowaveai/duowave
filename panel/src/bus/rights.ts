// Права приложения словами — одна функция на установку, окно приложения и мастерскую (docs/plans/bus.md, «Один
// способ»). С Host API 2 прав два вида: куда приложение ходит в интернет (`net:<хост>`) и до чего в доме дотягивается
// по шине (`see:/listen:/call:/edit:<адрес>`). Связи спрашиваются «да» при первом обращении, как в телефоне.

import { tr } from '../i18n';
import { byMode } from '../words';

/** Участники, которых люди знают по имени. Читаются при обращении: язык меняется без перезагрузки. */
const KNOWN: Record<string, { title: string; note: string }> = {
    'system/voice': {
        // Читается при отрисовке: в режиме «Микрофон» колонки нет.
        get title() {
            return byMode(tr.bus.knownVoiceStation, tr.bus.knownVoiceComputer);
        },
        get note() {
            return tr.bus.knownVoiceNote;
        },
    },
    'system/notify': {
        get title() {
            return tr.bus.knownNotify;
        },
        get note() {
            return tr.bus.knownNotifyNote;
        },
    },
    'system/schedule': {
        get title() {
            return tr.bus.knownSchedule;
        },
        get note() {
            return tr.bus.knownScheduleNote;
        },
    },
    'system/smart-home': {
        get title() {
            return tr.bus.knownSmartHome;
        },
        get note() {
            return tr.bus.knownSmartHomeNote;
        },
    },
    'system/content': {
        get title() {
            return tr.bus.knownContent;
        },
        get note() {
            return tr.bus.knownContentNote;
        },
    },
    'system/home': {
        get title() {
            return tr.bus.knownHome;
        },
        get note() {
            return tr.bus.knownHomeNote;
        },
    },
    'system/brain': {
        get title() {
            return tr.bus.knownBrain;
        },
        get note() {
            return tr.bus.knownBrainNote;
        },
    },
    'device/midi': {
        get title() {
            return tr.bus.knownMidi;
        },
        get note() {
            return tr.bus.knownMidiNote;
        },
    },
    'app/*': {
        get title() {
            return tr.bus.knownApps;
        },
        get note() {
            return tr.bus.knownAppsNote;
        },
    },
};

const verbs = (): Record<string, string> => ({ see: tr.bus.verbSee, listen: tr.bus.verbListen, call: tr.bus.verbCall, edit: tr.bus.verbEdit });
const fileVerbs = (): Record<string, string> => ({
    see: tr.bus.fileVerbSee,
    listen: tr.bus.fileVerbListen,
    call: tr.bus.fileVerbCall,
    edit: tr.bus.fileVerbEdit,
});

/** Право — для людей: что оно даёт, пояснение и уходит ли что-то из дома. */
export function rightWords(name: string): { title: string; note: string; out: boolean } {
    if (name.startsWith('net:')) return { title: tr.bus.netRight(name.slice(4)), note: '', out: true };
    const bus = /^(see|listen|call|edit):(.+)$/.exec(name);
    if (!bus) return { title: name, note: '', out: false };
    const [, level, pattern] = bus;
    const known = KNOWN[pattern];
    if (known) return { title: known.title, note: known.note, out: false };
    if (pattern.startsWith('files/')) {
        const folder = pattern.replace(/\/\*$/, '').split('/').pop() ?? pattern;
        return { title: tr.bus.folderRight(fileVerbs()[level] ?? level, folder), note: '', out: false };
    }
    if (pattern.startsWith('app/')) {
        const id = pattern.slice(4).replace(/\/\*$/, '');
        return { title: tr.bus.appRight(verbs()[level] ?? level, id), note: '', out: false };
    }
    return { title: `${verbs()[level] ?? level} ${pattern}`, note: '', out: false };
}

/** Право одной строкой: «Умный дом», «Ходить в интернет: api.example.com». */
export function rightTitle(name: string): string {
    return rightWords(name).title;
}
