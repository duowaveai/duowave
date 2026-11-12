// Распознавание на этом компьютере (docs/plans/speech-servers.md): модель скачивается при первом запуске, грузится
// при каждом старте шлюза и работает без сервера. Здесь — как это сказать человеку: строка состояния для экрана
// моделей и карточка на главной, пока слух готовится или упал.

import { useEffect, useState } from 'preact/hooks';

import { LocalAsr, api, can, problemOf } from '../api';
import { Progress } from '../ui/needs';

/** Мегабайты, как их считают люди и Hugging Face (274 МБ у GigaAM v3). */
const MB = 1_000_000;

/** На чём распознаёт: «видеокарте Мака», «процессоре»… */
function where(backend: string | null): string {
    switch ((backend ?? '').toLowerCase()) {
        case 'metal':
            return 'видеокарте Мака';
        case 'vulkan':
            return 'видеокарте';
        case 'cuda':
            return 'видеокарте NVIDIA';
        case 'rocm':
            return 'видеокарте AMD';
        case 'cpu':
        case 'cpu-accel':
            return 'процессоре';
        default:
            return backend ? backend : 'этом компьютере';
    }
}

/** Строка состояния: «работает на видеокарте Мака (Apple M4 Max)», «скачиваю модель · 120 из 274 МБ». */
export function localAsrLine(local: LocalAsr): string {
    switch (local.phase) {
        case 'downloading':
            return `скачиваю модель · ${Math.round(local.bytes / MB)} из ${Math.round(local.total / MB)} МБ`;
        case 'loading':
            return local.serving ? `загружаю новую модель, пока работает ${local.serving}` : 'загружаю модель…';
        case 'ready': {
            const device = local.device ? ` (${local.device})` : '';
            const fallback = local.gpuFailed ? ' — видеокарта отказала' : '';
            return `работает здесь, на ${where(local.backend)}${device}${fallback}`;
        }
        case 'failed':
            return local.serving ? `работает ${local.serving}; новая не встала` : 'не работает';
        default:
            return 'модель не выбрана';
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
                {failed ? 'Слух не встал' : 'Готовлю слух'}
            </h2>
            {local.phase === 'downloading' ? (
                <>
                    <p class="t-caption m-0">
                        Скачиваю модель распознавания речи — один раз. Дальше она работает на этом компьютере, без
                        интернета.
                    </p>
                    <Progress percent={localAsrPercent(local)} label="Модель распознавания" />
                    <p class="t-caption m-0">{localAsrLine(local)}</p>
                </>
            ) : failed ? (
                <>
                    <p class="t-caption m-0 c-dot-offline" role="alert">
                        {local.error ?? 'Распознавание не запустилось.'}
                    </p>
                    {local.serving && <p class="t-caption m-0">Пока слушает {local.serving}.</p>}
                    {can('models.manage') && (
                        <div>
                            <button class="w-btn w-btn-ink" disabled={retrying} onClick={() => void retry()}>
                                {retrying ? 'Пробую…' : 'Повторить'}
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
