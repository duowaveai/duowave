// Шина «Связи» словами для людей (docs/plans/bus.md): ступени, режимы, кто поставил связь, чем кончилось письмо.
// Те же слова, что у шлюза (gateway/src/bus/level.rs): у файлов «зовёт» — это «читает».

import type { BusLevel, BusLinkSource, BusMode } from '../api';
import { tr } from '../i18n';

/** Ступени по порядку: каждая держит те, что ниже. */
export const LEVELS: BusLevel[] = ['none', 'see', 'listen', 'call', 'edit'];

/** Ступень, как её читают о связи: «Покупки — зовёт — Заметки». */
export function levelWord(level: BusLevel, address = ''): string {
    const files = address.startsWith('files/');
    switch (level) {
        case 'none':
            return tr.bus.levelNone;
        case 'see':
            return tr.bus.levelSee;
        case 'listen':
            return tr.bus.levelListen;
        case 'call':
            return files ? tr.bus.levelCallFiles : tr.bus.levelCall;
        case 'edit':
            return files ? tr.bus.levelEditFiles : tr.bus.levelEdit;
    }
}

/** Что даёт ступень — подсказка к выбору. */
export function levelHint(level: BusLevel, address = ''): string {
    const files = address.startsWith('files/');
    switch (level) {
        case 'none':
            return files ? tr.bus.hintNoneFiles : tr.bus.hintNone;
        case 'see':
            return files ? tr.bus.hintSeeFiles : tr.bus.hintSee;
        case 'listen':
            return tr.bus.hintListen;
        case 'call':
            return files ? tr.bus.hintCallFiles : tr.bus.hintCall;
        case 'edit':
            return files ? tr.bus.hintEditFiles : tr.bus.hintEdit;
    }
}

// Словари читают язык при обращении: геттеры, не значения.
export const MODE_TITLES: Record<BusMode, string> = {
    get isolated() {
        return tr.bus.modeIsolated;
    },
    get rules() {
        return tr.bus.modeRules;
    },
    get free() {
        return tr.bus.modeFree;
    },
};

export const MODE_HINTS: Record<BusMode, string> = {
    get isolated() {
        return tr.bus.modeIsolatedHint;
    },
    get rules() {
        return tr.bus.modeRulesHint;
    },
    get free() {
        return tr.bus.modeFreeHint;
    },
};

export const SOURCE_TITLES: Record<BusLinkSource, string> = {
    get owner() {
        return tr.bus.sourceOwner;
    },
    get answer() {
        return tr.bus.sourceAnswer;
    },
    get refused() {
        return tr.bus.sourceRefused;
    },
    get install() {
        return tr.bus.sourceInstall;
    },
};

/** Чем кончилось письмо в журнале. */
export const OUTCOME_TITLES: Record<string, string> = {
    get ok() {
        return tr.bus.outcomeOk;
    },
    get failed() {
        return tr.bus.outcomeFailed;
    },
    get asked() {
        return tr.bus.outcomeAsked;
    },
    get denied() {
        return tr.bus.outcomeDenied;
    },
    get notFound() {
        return tr.bus.outcomeNotFound;
    },
    get fuse() {
        return tr.bus.outcomeFuse;
    },
    get yes() {
        return tr.bus.outcomeYes;
    },
    get no() {
        return tr.bus.outcomeNo;
    },
};

/** Что делали: позвал, рассказал новость, спросил словами, посмотрел, ответили, доставили. */
export const KIND_TITLES: Record<string, string> = {
    get call() {
        return tr.bus.kindCall;
    },
    get tell() {
        return tr.bus.kindTell;
    },
    get ask() {
        return tr.bus.kindAsk;
    },
    get list() {
        return tr.bus.kindList;
    },
    get answer() {
        return tr.bus.kindAnswer;
    },
    get deliver() {
        return tr.bus.kindDeliver;
    },
};

/** Опасности словами (gateway/src/bus/danger.rs). */
export const DANGER_TITLES: Record<string, string> = {
    get money() {
        return tr.bus.dangerMoney;
    },
    get delete() {
        return tr.bus.dangerDelete;
    },
    get outside() {
        return tr.bus.dangerOutside;
    },
    get locks() {
        return tr.bus.dangerLocks;
    },
    get camera() {
        return tr.bus.dangerCamera;
    },
    get access() {
        return tr.bus.dangerAccess;
    },
};

/** Кто начал цепочку, по метке журнала (gateway/src/bus/trail.rs): `user:…`, `household`, `apiKey:…`, `app:radio`,
 *  `system`. */
export function rootWord(root: string): string {
    if (root.startsWith('app:')) return tr.bus.rootApp(root.slice(4));
    if (root === 'system') return tr.bus.rootSystem;
    if (root === 'household') return tr.bus.rootHousehold;
    if (root.startsWith('user:')) return tr.bus.rootUser;
    if (root.startsWith('apiKey:')) return tr.bus.rootKey;
    return root;
}
