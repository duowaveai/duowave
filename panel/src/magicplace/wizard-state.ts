// Мастер установки (magicplace/wizard.tsx): кто его открыл и куда сказать, что поставлено. Отдельно от окна, чтобы
// «Установить» в каталоге (screens/catalog.tsx) открывало мастер без круга импортов.

import type { CatalogEntry } from '../api';
import { recalled, remember } from '../shell/memory';

export interface WizardOpening {
    entry: CatalogEntry;
    /** Сразу к шагам: продолжить начатое (с карточки приложения) — вопрос уже был. */
    resume: boolean;
    /** Что-то поставилось: экран, откуда открыли, перечитывает своё. */
    onChanged?: () => void;
    /** Мастер закрыли: поставлен ли сам пакет. */
    done: (installed: boolean | null) => void;
    /** Ставить у центра дома через проход помощника (ключ колонки), а не у своего шлюза (п. 42). */
    station?: string;
    /**
     * «Поставить» нажали раньше, пока колонка не была центром (п. 63): вопросов нет (одна модель, место есть, прав не
     * просит) — сразу шаги, свёрнуто, ход на пилюле; есть вопрос — мастер спрашивает, как обычно.
     */
    auto?: boolean;
}

const listeners = new Set<(opening: WizardOpening) => void>();

/** Открыть мастер на пакете каталога; обещание — поставлен ли сам пакет, когда мастер закрыли; null — мастера нет. */
export function openInstallWizard(
    entry: CatalogEntry,
    onChanged?: () => void,
    resume = false,
    station?: string,
    auto = false,
): Promise<boolean | null> {
    return new Promise((done) => {
        if (listeners.size === 0) {
            done(null);
            return;
        }
        for (const listener of [...listeners]) listener({ entry, resume, onChanged, done, station, auto });
    });
}

export function onInstallWizard(listener: (opening: WizardOpening) => void): () => void {
    listeners.add(listener);
    return () => {
        listeners.delete(listener);
    };
}

/** Начатая и не законченная установка — в памяти пульта у человека: мастер продолжается с карточки приложения. */
const KEY = 'install-wizard';

interface Unfinished {
    id: string;
    at: number;
}

export function unfinishedInstall(id: string): boolean {
    return recalled<Unfinished>(KEY)?.id === id;
}

export function markUnfinished(id: string | null): void {
    remember(KEY, id ? { id, at: Date.now() } : undefined);
}

/** Ход установки, пока мастер свёрнут: пилюля рядом с доком (InstallPill) показывает его и разворачивает мастер. */
export interface InstallProgress {
    title: string;
    done: number;
    total: number;
    finished: boolean;
    hidden: boolean;
    /** Что идёт сейчас с ходом: «модель 37 %» — пилюля говорит его вместо «3 из 5». */
    detail?: string;
    /** Какой пакет ставится: шаг «Персонаж» первых шагов показывает свёрнутую установку персонажа карточкой (п. 45). */
    kind?: string;
    id?: string;
    /** Его картинка — адрес, годный для этого пульта (у центра — через проход). */
    image?: string | null;
}

let progress: InstallProgress | null = null;
const progressListeners = new Set<() => void>();
const showListeners = new Set<() => void>();

export function setInstallProgress(next: InstallProgress | null): void {
    progress = next;
    for (const listener of [...progressListeners]) listener();
}

export function installProgress(): InstallProgress | null {
    return progress;
}

export function onInstallProgress(listener: () => void): () => void {
    progressListeners.add(listener);
    return () => {
        progressListeners.delete(listener);
    };
}

/** Развернуть свёрнутый мастер. */
export function showInstallWizard(): void {
    for (const listener of [...showListeners]) listener();
}

export function onShowInstallWizard(listener: () => void): () => void {
    showListeners.add(listener);
    return () => {
        showListeners.delete(listener);
    };
}
