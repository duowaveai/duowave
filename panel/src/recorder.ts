/**
 * Запись голоса для отпечатка: чистый звук микрофона через AudioWorklet, без обработки браузера (эхо-, шумоподавление и
 * автогромкость портят образец), в WAV 16 бит 24 кГц — так его ждёт шлюз. MediaRecorder не годится: Safari пишет AAC.
 * Голос пульта (voice/mic.ts) пишет тем же рекордером 16 кГц и с обработкой браузера: «Слуху» шлюза так понятнее.
 * Модуль ворклета — файл пульта (/panel/take-tap.js): политика безопасности пульта не пускает скрипты из blob-ссылок.
 * Не загрузился ворклет — пишет старый ScriptProcessor: звук тот же. Файл с диктофона браузер раскодирует сам.
 */

/** Модуль ворклета записи: лежит в panel/public, отдаётся с адреса пульта. */
const WORKLET_URL = '/panel/take-tap.js';

export const TAKE_RATE = 24_000;
/** Запись останавливается сама: отрывок читается секунд за сорок. */
export const MAX_TAKE_SECONDS = 60;

/** Можно ли записывать здесь: микрофон браузер даёт только на localhost и по HTTPS. */
export function recordingSupport(): { ok: boolean; reason?: string } {
    if (!window.isSecureContext)
        return {
            ok: false,
            reason: 'Микрофон браузер даёт только на самом компьютере со шлюзом (localhost) или по HTTPS. Здесь можно добавить файл: запишите отрывок диктофоном и загрузите.',
        };
    if (!navigator.mediaDevices?.getUserMedia || typeof AudioContext === 'undefined')
        return { ok: false, reason: 'Этот браузер не умеет записывать звук. Добавьте файл с диктофона.' };
    return { ok: true };
}

/** Микрофоны: имена браузер показывает после первого разрешения. */
export async function microphones(): Promise<Array<{ id: string; label: string }>> {
    const devices = await navigator.mediaDevices.enumerateDevices();
    return devices
        .filter((device) => device.kind === 'audioinput')
        .map((device, index) => ({ id: device.deviceId, label: device.label || `Микрофон ${index + 1}` }));
}

/** 16-битный моно WAV. */
export function encodeWav(samples: Float32Array, rate: number): Blob {
    const data = new DataView(new ArrayBuffer(44 + samples.length * 2));
    const text = (offset: number, value: string) => [...value].forEach((char, i) => data.setUint8(offset + i, char.charCodeAt(0)));
    text(0, 'RIFF');
    data.setUint32(4, 36 + samples.length * 2, true);
    text(8, 'WAVE');
    text(12, 'fmt ');
    data.setUint32(16, 16, true);
    data.setUint16(20, 1, true);
    data.setUint16(22, 1, true);
    data.setUint32(24, rate, true);
    data.setUint32(28, rate * 2, true);
    data.setUint16(32, 2, true);
    data.setUint16(34, 16, true);
    text(36, 'data');
    data.setUint32(40, samples.length * 2, true);
    for (let i = 0; i < samples.length; i++) {
        const value = Math.max(-1, Math.min(1, samples[i]));
        data.setInt16(44 + i * 2, value < 0 ? value * 32768 : value * 32767, true);
    }
    return new Blob([data.buffer], { type: 'audio/wav' });
}

/** Звук в нужной частоте (24 кГц отпечатка, 16 кГц «Слуха») моно — частоту меняет сам браузер, с нормальным фильтром. */
async function toTakeRate(buffer: AudioBuffer, rate = TAKE_RATE): Promise<Float32Array> {
    const mono = new Float32Array(buffer.length);
    for (let channel = 0; channel < buffer.numberOfChannels; channel++) {
        const data = buffer.getChannelData(channel);
        for (let i = 0; i < data.length; i++) mono[i] += data[i] / buffer.numberOfChannels;
    }
    if (buffer.sampleRate === rate) return mono;
    const offline = new OfflineAudioContext(1, Math.ceil((mono.length * rate) / buffer.sampleRate), rate);
    const source = offline.createBufferSource();
    const input = offline.createBuffer(1, mono.length, buffer.sampleRate);
    input.copyToChannel(mono, 0);
    source.buffer = input;
    source.connect(offline.destination);
    source.start();
    return (await offline.startRendering()).getChannelData(0);
}

/** Файл с диктофона (m4a, mp3, wav…) в WAV для шлюза. */
export async function fileToWav(file: File): Promise<{ wav: Blob; seconds: number }> {
    const context = new OfflineAudioContext(1, 1, TAKE_RATE);
    let decoded: AudioBuffer;
    try {
        decoded = await context.decodeAudioData(await file.arrayBuffer());
    } catch {
        throw new Error('Не получилось прочитать файл: нужен звук — m4a, mp3, wav.');
    }
    const samples = await toTakeRate(decoded);
    return { wav: encodeWav(samples, TAKE_RATE), seconds: samples.length / TAKE_RATE };
}

/** Как писать: частота WAV и обработка браузера (эхо- и шумоподавление, автогромкость) — у голоса пульта она есть. */
export interface RecordAs {
    rate?: number;
    processed?: boolean;
}

/** Одна запись с микрофона: start() — в обработчике нажатия (Safari иначе не даст звук), stop() отдаёт WAV. */
export class Recorder {
    private readonly chunks: Float32Array[] = [];
    private length = 0;
    private detach: () => void = () => undefined;

    private constructor(
        private readonly context: AudioContext,
        private readonly stream: MediaStream,
        private readonly rate: number,
    ) {}

    static async start(deviceId: string | undefined, onLevel: (db: number, seconds: number) => void, as: RecordAs = {}): Promise<Recorder> {
        const rate = as.rate ?? TAKE_RATE;
        let context: AudioContext;
        try {
            context = new AudioContext({ sampleRate: rate });
        } catch {
            context = new AudioContext(); // частоту приведём на остановке
        }
        void context.resume();
        let stream: MediaStream;
        try {
            stream = await navigator.mediaDevices.getUserMedia({
                audio: {
                    ...(deviceId ? { deviceId: { exact: deviceId } } : {}),
                    channelCount: 1,
                    echoCancellation: as.processed === true,
                    noiseSuppression: as.processed === true,
                    autoGainControl: as.processed === true,
                },
            });
        } catch (error) {
            void context.close();
            const name = (error as DOMException).name;
            throw new Error(
                name === 'NotAllowedError'
                    ? 'Браузер не дал микрофон — разрешите доступ к нему в настройках сайта.'
                    : name === 'NotFoundError'
                      ? 'Микрофон не найден — подключите его и попробуйте снова.'
                      : `Микрофон не открылся: ${(error as Error).message}`,
            );
        }
        const recorder = new Recorder(context, stream, rate);
        const take = (chunk: Float32Array): void => {
            recorder.chunks.push(chunk);
            recorder.length += chunk.length;
            let energy = 0;
            for (let i = 0; i < chunk.length; i++) energy += chunk[i] * chunk[i];
            onLevel(20 * Math.log10(Math.sqrt(energy / chunk.length) + 1e-9), recorder.length / context.sampleRate);
        };
        const source = context.createMediaStreamSource(stream);
        const silent = context.createGain();
        silent.gain.value = 0; // узел тянет звук, но в колонки ничего не идёт
        silent.connect(context.destination);
        try {
            await context.audioWorklet.addModule(WORKLET_URL);
            const node = new AudioWorkletNode(context, 'take-tap');
            node.port.onmessage = (event: MessageEvent<Float32Array>) => take(event.data);
            source.connect(node).connect(silent);
            recorder.detach = () => {
                node.port.onmessage = null;
                node.disconnect();
            };
        } catch {
            // Старый, но везде живой путь: звук тот же, только обрабатывается в основном потоке.
            const node = context.createScriptProcessor(4096, 1, 1);
            node.onaudioprocess = (event) => take(event.inputBuffer.getChannelData(0).slice(0));
            source.connect(node).connect(silent);
            recorder.detach = () => {
                node.onaudioprocess = null;
                node.disconnect();
            };
        }
        return recorder;
    }

    get seconds(): number {
        return this.length / this.context.sampleRate;
    }

    async stop(): Promise<{ wav: Blob; seconds: number }> {
        const rate = this.context.sampleRate;
        const wanted = this.rate;
        this.release();
        const samples = new Float32Array(this.length);
        let offset = 0;
        for (const chunk of this.chunks) {
            samples.set(chunk, offset);
            offset += chunk.length;
        }
        if (rate === wanted) return { wav: encodeWav(samples, wanted), seconds: samples.length / wanted };
        const buffer = new AudioBuffer({ length: Math.max(1, samples.length), numberOfChannels: 1, sampleRate: rate });
        buffer.copyToChannel(samples, 0);
        const converted = await toTakeRate(buffer, wanted);
        return { wav: encodeWav(converted, wanted), seconds: converted.length / wanted };
    }

    cancel(): void {
        this.release();
    }

    private release(): void {
        this.detach();
        this.stream.getTracks().forEach((track) => track.stop());
        void this.context.close();
    }
}
