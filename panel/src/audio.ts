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
