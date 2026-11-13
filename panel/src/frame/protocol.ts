// Окошко приложения (docs/plans/bus.md, «Окошки»): код пульта не встроенного приложения работает в изолированном
// фрейме — `sandbox="allow-scripts"` без своего происхождения, `connect-src 'none'` (gateway/src/panel/controller.rs).
// В шлюз и к другим он ходит только через пульт: сообщения ниже. Пульт знает, чьё окошко, по самому фрейму, а не по
// словам в сообщении: приложение говорит только за себя.

/** Что показать в окошке: узнать, что приложение приносит (разведка), его раздел или вклад в слот. */
export type FramePart = { kind: 'probe' } | { kind: 'section'; id: string } | { kind: 'slot'; name: string; id: string };

/** Что пульт рассказывает окошку о себе: только то, что приложению можно знать. */
export interface FrameState {
    app: string;
    /** Номер перемены его пакета (plugins/versions.ts): перечитать своё. */
    version: number;
    settings: Record<string, unknown>;
    permissions: string[];
    station: { device: string | null; name: string; online: boolean; mode: string | null; role: 'gateway' | 'helper' };
    /** Канал пульта открыт ли и как слышно колонки — без запросов людей. */
    live: { connected: boolean; presence: Record<string, unknown> };
    /** Тема: правило токенов и атрибуты <html>. */
    theme: { css: string; attrs: Record<string, string> };
    /** Черновик на примере: клавиатура — экранная, сеть — пример. */
    draft: boolean;
}

/** Раздел или вклад, как окошко его описывает пульту: без кода, только слова и значок по имени. */
export interface FrameSection {
    id: string;
    title: string;
    icon: string;
    order?: number;
    nav?: 'main' | 'more' | 'link';
    permission?: string;
}

export interface FrameSlotEntry {
    id: string;
    title?: string;
    icon?: string;
    order?: number;
    permission?: string;
    size?: 'small' | 'large';
    source?: string;
    initial?: boolean;
    roles?: Array<'gateway' | 'helper'>;
    bare?: boolean;
}

export interface FrameShape {
    sections: FrameSection[];
    slots: Record<string, FrameSlotEntry[]>;
}

/** Слоты, куда окошко может принести своё: стол, главная, вкладки «Дома». */
export const FRAME_SLOTS = ['desk.widgets', 'home.cards', 'house.tabs'];

/** Пульт → окошко. */
export type ToFrame =
    | { type: 'duowave:init'; module: string; styles?: string | null; part: FramePart; state: FrameState }
    | { type: 'duowave:state'; state: FrameState }
    | { type: 'duowave:answer'; seq: number; ok: boolean; value?: unknown; error?: string }
    | { type: 'duowave:midi-state'; state: FrameMidiState }
    | { type: 'duowave:midi-event'; event: unknown };

/** Окошко → пульт. */
export type FromFrame =
    | { type: 'duowave:ready' }
    | { type: 'duowave:shape'; shape: FrameShape }
    | { type: 'duowave:failed'; message: string }
    | { type: 'duowave:size'; height: number }
    | { type: 'duowave:call'; seq: number; fn: string; input: unknown }
    | { type: 'duowave:file'; seq: number; name: string }
    | { type: 'duowave:midi-file'; seq: number; file: Blob }
    | { type: 'duowave:ask'; seq: number; options: { title: string; text?: string; ok?: string; danger?: boolean } }
    | { type: 'duowave:go'; section: string; tab?: string; item?: string }
    | { type: 'duowave:midi'; op: string; args?: unknown[] };

/** Клавиатура приложения, как её видит окошко: то же, что `useMidi()` в пульте, без методов. */
export interface FrameMidiState {
    state: string;
    via: string;
    why: string | null;
    inputs: Array<{ id: string; name: string }>;
    outputs: Array<{ id: string; name: string }>;
    input: { id: string; name: string } | null;
    output: { id: string; name: string } | null;
    delayMs: number | null;
    browserPossible: boolean;
}

/** Сообщение — наше: объект с `type`, начинающимся с «duowave:». */
export function ours(data: unknown): data is { type: string } {
    return !!data && typeof data === 'object' && typeof (data as { type?: unknown }).type === 'string' && (data as { type: string }).type.startsWith('duowave:');
}
