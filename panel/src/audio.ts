import { useEffect, useState } from 'preact/hooks';

/**
 * Проигрывание звука пульта — дублей и проб голоса: звучит одно за раз, новое останавливает прежнее. Звук приходит
 * blob'ом (его отдают с ключом входа), ссылка на него освобождается, когда он доиграл или его остановили.
 */

let current: { key: string; audio: HTMLAudioElement; url: string } | undefined;
const listeners = new Set<(key: string | null) => void>();

function settle(key: string | null): void {
    listeners.forEach((listener) => listener(key));
}

export function stopAudio(): void {
    if (!current) return;
    current.audio.pause();
    URL.revokeObjectURL(current.url);
    current = undefined;
    settle(null);
}

export async function playAudio(key: string, load: () => Promise<Blob>): Promise<void> {
    stopAudio();
    const url = URL.createObjectURL(await load());
    const audio = new Audio(url);
    current = { key, audio, url };
    audio.onended = () => current?.key === key && stopAudio();
    settle(key);
    try {
        await audio.play();
    } catch {
        stopAudio();
    }
}

/**
 * Проиграть звук до конца: обещание кончается, когда он доиграл (true) или его остановили — другим звуком, `stopAudio`
 * (false). Так пульт говорит ответ по фразам: следующая — по концу предыдущей (voice/speak.ts).
 */
export function playToEnd(key: string, blob: Blob): Promise<boolean> {
    stopAudio();
    const url = URL.createObjectURL(blob);
    const audio = new Audio(url);
    const mine = { key, audio, url };
    current = mine;
    settle(key);
    return new Promise((resolve) => {
        let done = false;
        const finish = (ended: boolean): void => {
            if (done) return;
            done = true;
            listeners.delete(watch);
            if (current === mine) stopAudio();
            resolve(ended);
        };
        // Остановили: звучит уже другое или ничего.
        const watch = (): void => {
            if (current !== mine) finish(false);
        };
        listeners.add(watch);
        audio.onended = () => finish(true);
        audio.onerror = () => finish(false);
        audio.play().catch(() => finish(false));
    });
}

/** Что сейчас звучит (ключ) и переключатель «играть / стоп». */
export function useAudio(): { playing: string | null; toggle: (key: string, load: () => Promise<Blob>) => Promise<void> } {
    const [playing, setPlaying] = useState<string | null>(current?.key ?? null);
    useEffect(() => {
        listeners.add(setPlaying);
        return () => {
            listeners.delete(setPlaying);
        };
    }, []);
    return {
        playing,
        toggle: async (key, load) => (current?.key === key ? stopAudio() : playAudio(key, load)),
    };
}
