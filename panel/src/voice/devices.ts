// Откуда пульт слушает и куда говорит (просьба владельца при проверке второго этапа, 15.11): микрофон и выход звука
// выбираются в «Настройки» → «Микрофон». По умолчанию — «Автоматически»: что даёт система (иначе в наушниках, которые
// оказались подключены, а слушать хотели встроенный микрофон). Выбор хранится в этом браузере (окне приложения).
// Выбранного устройства нет (отключили наушники) — берётся системное, молча.
// Выход звука браузер даёт выбрать не везде (`setSinkId`): где нельзя, выбор не показывается — выход задаёт система.

const INPUT_KEY = 'duowave.micInput';
const OUTPUT_KEY = 'duowave.soundOutput';

export interface AudioDevice {
    id: string;
    label: string;
}

function read(key: string): string {
    try {
        return localStorage.getItem(key) ?? '';
    } catch {
        return '';
    }
}

function write(key: string, value: string): void {
    try {
        if (value) localStorage.setItem(key, value);
        else localStorage.removeItem(key);
    } catch {
        // Хранилище закрыто — остаётся «Автоматически».
    }
}

/** Выбранный микрофон; пусто — системный. */
export function chosenInput(): string {
    return read(INPUT_KEY);
}

export function setChosenInput(id: string): void {
    write(INPUT_KEY, id);
}

/** Выбранный выход звука; пусто — системный. */
export function chosenOutput(): string {
    return read(OUTPUT_KEY);
}

export function setChosenOutput(id: string): void {
    write(OUTPUT_KEY, id);
}

/** Можно ли здесь выбрать, куда идёт звук. */
export function outputChoosable(): boolean {
    return typeof HTMLMediaElement !== 'undefined' && 'setSinkId' in HTMLMediaElement.prototype;
}

/** Входы и выходы звука; названия браузер показывает после первого разрешения микрофона. «default» — это «Автоматически». */
export async function audioDevices(): Promise<{ inputs: AudioDevice[]; outputs: AudioDevice[] }> {
    const devices = await navigator.mediaDevices.enumerateDevices();
    const of = (kind: MediaDeviceKind): AudioDevice[] =>
        devices
            .filter((device) => device.kind === kind && device.deviceId && device.deviceId !== 'default' && device.deviceId !== 'communications')
            .map((device, index) => ({ id: device.deviceId, label: device.label || `${kind === 'audioinput' ? '🎙' : '🔈'} ${index + 1}` }));
    return { inputs: of('audioinput'), outputs: of('audiooutput') };
}

/** Направить звук элемента в выбранный выход, если он выбран и это можно. Не вышло — остаётся системный. */
export async function routeOutput(audio: HTMLAudioElement): Promise<void> {
    const id = chosenOutput();
    if (!id || !outputChoosable()) return;
    try {
        await (audio as HTMLAudioElement & { setSinkId(id: string): Promise<void> }).setSinkId(id);
    } catch {
        // Выхода нет (отключили) — играет системный.
    }
}

/** То же для звуков микрофона (WebAudio), где браузер это умеет. */
export function routeContext(context: AudioContext): void {
    const id = chosenOutput();
    const sink = context as AudioContext & { setSinkId?: (id: string) => Promise<void> };
    if (!id || typeof sink.setSinkId !== 'function') return;
    sink.setSinkId(id).catch(() => undefined);
}
