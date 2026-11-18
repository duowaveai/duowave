// Персонаж донастраивается (проверка начисто 18.11, п. 67). Мастер установки кончил свои шаги — персонаж и его слово
// стоят, — а голос ещё ставится на компьютере (модель скачивается, гигабайты): пилюля пропадала, и владелец не понимал,
// завис ли он. Теперь мастер, отпуская голос «догонять самому», запоминает персонажа здесь (в браузере); пока его голос
// не включён, пилюля у дока (и в шапке первых шагов) говорит ход: «ГЛаДОС донастраивается · модель 72 %», «скачивание
// стоит — повторяю»; не вышло — словами. Нажатие — окно с ходом и, если модель не встала, «Повторить».

import { useEffect, useState } from 'preact/hooks';

import { HomeVoicePlan, api, installApi, problemOf } from '../api';
import { tr } from '../i18n';
import { usePanel } from '../shell/context';
import { Sheet, WinRow } from '../ui/sheet';

/** Кого ждём: пакет персонажа, его название; у компьютера-помощника — ключ колонки (план голоса — у центра). */
interface Settling {
    id: string;
    title: string;
    station?: string;
    at: number;
}

/** Что сейчас, словами и цифрами. */
export interface SettlingView {
    name: string;
    /** model — скачивается модель; voice — голос под неё; wait — голос включается; away — компьютер не на связи. */
    stage: 'model' | 'voice' | 'wait' | 'away';
    percent: number | null;
    detail: string | null;
    /** Модель не встала — словами шлюза. */
    failed: string | null;
}

const KEY = 'voice-settling';
const READ_EVERY_MS = 3000;
/** Дольше суток не ждём: выбор устарел (персонажа сменили, компьютер убран). */
const KEEP_MS = 24 * 60 * 60_000;

let settling: Settling | null = (() => {
    try {
        const kept = JSON.parse(localStorage.getItem(KEY) ?? 'null');
        return kept && typeof kept.id === 'string' && Date.now() - kept.at < KEEP_MS ? (kept as Settling) : null;
    } catch {
        return null;
    }
})();
let view: SettlingView | null = null;
let open = false;
const listeners = new Set<() => void>();

function notify(): void {
    for (const listener of [...listeners]) listener();
}

function keep(next: Settling | null): void {
    settling = next;
    if (!next) {
        view = null;
        open = false;
    }
    try {
        if (next) localStorage.setItem(KEY, JSON.stringify(next));
        else localStorage.removeItem(KEY);
    } catch {
        // приватное окно — помним до перезагрузки
    }
    notify();
}

/** Имя без пометки: «ГЛаДОС (демо)» → «ГЛаДОС». */
const plain = (title: string): string => title.replace(/\s*\([^)]*\)\s*$/, '').trim() || title;

/** Мастер отпустил голос персонажа догонять самому: следим, пока он не включится. */
export function settleVoice(entry: { id: string; title: string }, station?: string): void {
    view = { name: plain(entry.title), stage: 'model', percent: null, detail: null, failed: null };
    keep({ id: entry.id, title: entry.title, station, at: Date.now() });
}

/** «Не ждать»: пилюля уходит, голос ставится как ставился. */
export function dropSettling(): void {
    keep(null);
}

export function showSettling(): void {
    open = true;
    notify();
}

function useSettlingState(): { settling: Settling | null; view: SettlingView | null; open: boolean } {
    const [, rerender] = useState(0);
    useEffect(() => {
        const listener = () => rerender((n) => n + 1);
        listeners.add(listener);
        return () => {
            listeners.delete(listener);
        };
    }, []);
    return { settling, view, open };
}

export function useSettling(): SettlingView | null {
    return useSettlingState().view;
}

/** Пилюля: «ГЛаДОС донастраивается · модель 72 %», «… · скачивание стоит — повторяю», «ГЛаДОС: голос не встал». */
export function settlingLine(now: SettlingView): string {
    const t = tr.wizard;
    if (now.failed) return t.settlingFailed(now.name);
    if (now.stage === 'away') return t.settlingAway(now.name);
    if (now.detail && /стоял|stood|stalled/i.test(now.detail)) return t.settlingStalled(now.name);
    if (now.stage === 'model' && now.percent !== null) return t.settlingModel(now.name, now.percent);
    if (now.stage === 'voice' && now.percent !== null) return t.settlingVoice(now.name, now.percent);
    return t.settling(now.name);
}

/** План голоса → что сейчас; голос включён — null (ждать нечего). */
function read(plan: HomeVoicePlan, name: string): SettlingView | null {
    const voice = plan.plan;
    if (voice?.voiceOn) return null;
    if (!plan.turbo?.up || !voice) return { name, stage: 'away', percent: null, detail: null, failed: null };
    const going = voice.progress;
    if (going)
        return { name, stage: going.stage, percent: going.percent ?? null, detail: going.detail ?? null, failed: null };
    return { name, stage: 'wait', percent: null, detail: null, failed: voice.failed ?? null };
}

/** Смотрит план голоса, пока персонаж донастраивается; стоит рядом с мастером (app.tsx) — на любом экране. */
export function SettlingRunner() {
    const { settling: now, open: shown } = useSettlingState();
    useEffect(() => {
        if (!now) return;
        let alive = true;
        let timer: ReturnType<typeof setTimeout> | undefined;
        const look = async (): Promise<void> => {
            const plan = await installApi(now.station)
                .homeVoicePlan({ package: now.id })
                .catch(() => null);
            if (!alive || settling?.id !== now.id) return;
            if (plan) {
                const next = read(plan, plain(now.title));
                if (!next) return keep(null);
                view = next;
                notify();
            }
            timer = setTimeout(() => void look(), READ_EVERY_MS);
        };
        void look();
        return () => {
            alive = false;
            if (timer) clearTimeout(timer);
        };
    }, [now?.id, now?.at]);
    return shown && now && view ? <SettlingSheet now={view} /> : null;
}

/** Окно хода: модель (процент, мегабайты, скорость), голос; не встала — словами и «Повторить». */
function SettlingSheet({ now }: { now: SettlingView }) {
    const t = tr.wizard;
    const panel = usePanel();
    const [busy, setBusy] = useState(false);
    const [problem, setProblem] = useState<string | null>(null);
    const close = (): void => {
        open = false;
        notify();
    };
    // Повторить можно там, где ставится модель: этот компьютер и есть Турбо (помощник) — его отложенный голос.
    const mayRetry = !!now.failed && panel.role.role === 'helper';
    const retry = async (): Promise<void> => {
        setBusy(true);
        setProblem(null);
        try {
            const pending = (await api.models()).ttsPending;
            if (!pending) throw new Error(t.settlingNothingToRetry);
            await api.activateTts(pending.modelId, pending.voiceId ?? undefined, false, true);
            if (view) view = { ...view, failed: null, stage: 'model', percent: 0, detail: null };
            notify();
        } catch (e) {
            setProblem(problemOf(e));
        } finally {
            setBusy(false);
        }
    };
    const model =
        now.stage === 'model'
            ? [now.percent !== null ? `${now.percent} %` : null, now.detail].filter(Boolean).join(' · ') || t.settlingModelWait
            : now.stage === 'away'
              ? t.settlingAwayLong
              : t.settlingModelDone;
    return (
        <Sheet
            title={now.failed ? t.settlingFailed(now.name) : t.settlingTitle(now.name)}
            about={t.settlingAbout}
            onClose={close}
            footer={
                <>
                    <button class="btn btn-quiet" onClick={() => (dropSettling(), close())}>
                        {t.settlingDrop}
                    </button>
                    {mayRetry && (
                        <button class="btn btn-plain" disabled={busy} onClick={() => void retry()}>
                            {t.retry}
                        </button>
                    )}
                    <button class="btn btn-main" onClick={close}>
                        {t.close}
                    </button>
                </>
            }
        >
            <div class="win-group" role="list">
                <div role="listitem">
                    <WinRow title={t.settlingModelRow} sub={now.failed ?? model} />
                </div>
                <div role="listitem">
                    <WinRow
                        title={t.settlingVoiceRow}
                        sub={now.stage === 'voice' && now.percent !== null ? `${now.percent} %` : now.stage === 'wait' && !now.failed ? t.settlingVoiceOn : t.settlingVoiceLater}
                    />
                </div>
            </div>
            {problem && (
                <p class="win-note m-0" role="alert">
                    {problem}
                </p>
            )}
        </Sheet>
    );
}
