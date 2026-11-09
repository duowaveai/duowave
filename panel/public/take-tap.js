// Запись дубля для отпечатка голоса: чистый звук микрофона кусками по 200 мс (4800 сэмплов при 24 кГц).
// Отдельный файл с адреса пульта: его политика безопасности не пускает скрипты из blob-ссылок.
class TakeTap extends AudioWorkletProcessor {
    constructor() {
        super();
        this.buffer = new Float32Array(4800);
        this.filled = 0;
    }

    process(inputs) {
        const channel = inputs[0] && inputs[0][0];
        if (channel) {
            for (let i = 0; i < channel.length; i++) {
                this.buffer[this.filled++] = channel[i];
                if (this.filled === this.buffer.length) {
                    this.port.postMessage(this.buffer.slice(0));
                    this.filled = 0;
                }
            }
        }
        return true;
    }
}

registerProcessor('take-tap', TakeTap);
