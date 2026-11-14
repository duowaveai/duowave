// Распознавание на этом компьютере (docs/plans/speech-servers.md): модель скачивается при первом запуске, грузится
// при каждом старте шлюза и работает без сервера. Здесь — как это сказать человеку: строка состояния для экрана
// моделей и карточка на главной, пока слух готовится или упал.

import { useEffect, useState } from 'preact/hooks';

import { LocalAsr, api, can, problemOf } from '../api';
import { Progress } from '../ui/needs';
import { tr } from '../i18n';

/** Мегабайты, как их считают люди и Hugging Face (274 МБ у GigaAM v3). */
const MB = 1_000_000;

/** На чём распознаёт: «видеокарте Мака», «процессоре»… */
function where(backend: string | null): string {
    const t = tr.localAsr;
    switch ((backend ?? '').toLowerCase()) {
        case 'metal':
            return t.backendMetal;
        case 'vulkan':
            return t.backendVulkan;
        case 'cuda':
            return t.backendCuda;
        case 'rocm':
            return t.backendRocm;
        case 'cpu':
        case 'cpu-accel':
            return t.backendCpu;
        default:
            return backend ? backend : t.backendThis;
    }
}

/** Строка состояния: «работает на видеокарте Мака (Apple M4 Max)», «скачиваю модель · 120 из 274 МБ». */
export function localAsrLine(local: LocalAsr): string {
    const t = tr.localAsr;
    switch (local.phase) {
        case 'downloading':
            return t.downloading(Math.round(local.bytes / MB), Math.round(local.total / MB));
        case 'loading':
            return local.serving ? t.loadingWhile(local.serving) : t.loading;
        case 'ready': {
            const device = local.device ? ` (${local.device})` : '';
            const fallback = local.gpuFailed ? t.gpuFailed : '';
            return t.readyHere(where(local.backend), device, fallback);
        }
        case 'failed':
            return local.serving ? t.failedServing(local.serving) : t.failed;
        default:
            return t.noModel;
    }
}

/** Доля скачанного, %. */
export function localAsrPercent(local: LocalAsr): number {
    return local.total > 0 ? (local.bytes / local.total) * 100 : 0;
}

/** Распознавание на этом компьютере, как его видит главная: часто, пока не работает; изредка — потом (вдруг упадёт).
 * null — распознаёт сервер или шлюз ещё не ответил. */
export function useLocalAsr(): [LocalAsr | null, (next: LocalAsr | null) => void] {
    const [local, setLocal] = useState<LocalAsr | null>(null);
    useEffect(() => {
        let alive = true;
        let timer: ReturnType<typeof setTimeout> | undefined;
        const read = () => {
            api.localAsr()
                .then(({ local: fresh }) => {
                    if (!alive) return;
                    setLocal(fresh);
                    if (fresh) timer = setTimeout(read, fresh.phase !== 'ready' ? 1500 : 30_000);
                })
                .catch(() => {
                    // Шлюз мог ещё не подняться (первый запуск, перезапуск): скоро спросим снова.
                    if (alive) timer = setTimeout(read, 3_000);
                });
        };
        read();
        return () => {
            alive = false;
            if (timer) clearTimeout(timer);
        };
    }, []);
    return [local, setLocal];
}

/** Показывать ли «Готовлю слух»: слуха сейчас нет — модель качается, грузится или упала. Пока новая модель встаёт, а
 * старая слушает, тревожиться не о чем: это видно на экране моделей, не на главной. */
export function localAsrNeedsCard(local: LocalAsr | null): local is LocalAsr {
    return !!local && !local.serving && local.phase !== 'ready' && local.phase !== 'idle';
}

/** «Готовлю слух»: на главной, пока модель качается, грузится или упала; когда слух работает — её нет. */
export function LocalAsrSetup({ local, onChange }: { local: LocalAsr; onChange: (next: LocalAsr | null) => void }) {
    const [retrying, setRetrying] = useState(false);
    const [problem, setProblem] = useState<string | null>(null);
    const retry = async () => {
        setRetrying(true);
        setProblem(null);
        try {
            await api.restartServer('asr');
            onChange((await api.localAsr()).local);
        } catch (e) {
            setProblem(problemOf(e));
        } finally {
            setRetrying(false);
        }
    };
    const failed = local.phase === 'failed';
    return (
        <section class="card glass first-run" aria-labelledby="local-asr-h">
            <h2 id="local-asr-h" class="t-title m-0">
                {failed ? tr.localAsr.titleFailed : tr.localAsr.titleSetup}
            </h2>
            {local.phase === 'downloading' ? (
                <>
                    <p class="t-caption m-0">
                        {tr.localAsr.downloadingNote}
                    </p>
                    <Progress percent={localAsrPercent(local)} label={tr.localAsr.progressLabel} />
                    <p class="t-caption m-0">{localAsrLine(local)}</p>
                </>
            ) : failed ? (
                <>
                    <p class="t-caption m-0 c-dot-offline" role="alert">
                        {local.error ?? tr.localAsr.notStarted}
                    </p>
                    {local.serving && <p class="t-caption m-0">{tr.localAsr.listening(local.serving)}</p>}
                    {can('models.manage') && (
                        <div>
                            <button class="w-btn w-btn-ink" disabled={retrying} onClick={() => void retry()}>
                                {retrying ? tr.localAsr.retrying : tr.localAsr.retry}
                            </button>
                        </div>
                    )}
                    {problem && (
                        <p class="t-caption m-0 c-dot-offline" role="alert">
                            {problem}
                        </p>
                    )}
                </>
            ) : (
                <p class="t-caption m-0">{localAsrLine(local)}</p>
            )}
        </section>
    );
}
