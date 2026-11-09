// «Нужна модель»: голос сам знает, чего ему не хватает (docs/plans/characters.md, раздел 5). Одно окно для всех —
// персонажи, свои голоса, Piper и Silero: сколько скачать, сколько займёт, сколько ждать, «Скачать и включить» и
// «Пока говорить голосом …». Тут же — метки состояния голоса для карточек и плиток.

import { useState } from 'preact/hooks';

import type { CharacterVoice, TtsCard, VoiceNeeds } from '../api';
import { api, isTight, plural, problemOf } from '../api';
import { Sheet } from './sheet';

const decimal = (value: number): string => String(value).replace('.', ',');

/** «60 МБ», «3,1 ГБ», «10 ГБ». */
export function sizeText(mb: number): string {
    if (mb < 1024) return `${Math.max(1, Math.round(mb))} МБ`;
    const gb = mb / 1024;
    return `${decimal(gb >= 10 ? Math.round(gb) : Math.round(gb * 10) / 10)} ГБ`;
}

/** Сколько займёт на диске (а не известно — сколько скачать). */
export function needsSize(needs?: VoiceNeeds | null): string | null {
    const mb = needs?.diskMb ?? needs?.downloadMb;
    return mb ? sizeText(mb) : null;
}

/** «модель OmniVoice», «файл голоса». */
export function needsWhat(needs: VoiceNeeds): string {
    return needs.kind === 'engine' ? `модель ${needs.title ?? needs.id}` : 'файл голоса';
}

/** Метка готовности голоса: нужна модель, качается, не пойдёт здесь. Готовому голосу метка не нужна. */
export function voicePill(voice: Pick<TtsCard, 'state' | 'progress' | 'needs'> | CharacterVoice | null | undefined): { text: string; cls: string } | null {
    if (!voice?.state) return null;
    if (voice.state === 'needs') {
        const size = needsSize(voice.needs);
        const what = voice.needs?.kind === 'file' ? 'нужен файл' : 'нужна модель';
        return { text: size ? `${what} · ${size}` : what, cls: 'pill-yandex' };
    }
    if (voice.state === 'installing')
        return { text: voice.progress !== undefined ? `качается ${Math.round(voice.progress)} %` : 'качается', cls: 'pill-off' };
    if (voice.state === 'unsupported') return { text: 'не пойдёт здесь', cls: 'pill-off' };
    return null;
}

/** Полоса хода: установка модели, скачивание файла. Ширина — данные, поэтому инлайн (как у progress-fill везде). */
export function Progress({ percent, label }: { percent: number; label: string }) {
    const value = Math.max(0, Math.min(100, Math.round(percent)));
    return (
        <div class="flex items-center gap-10">
            <div role="progressbar" aria-label={label} aria-valuenow={value} class="flex-1 h-8 r-pill bg-track overflow-hidden">
                <div class="progress-fill" style={{ width: `${Math.max(3, value)}%` }} />
            </div>
            <span class="t-caption fw-700 flex-none">{value} %</span>
        </div>
    );
}

/** «Скачать ~3 ГБ, на диске ~4 ГБ, около 2 минут.» — что известно. */
function facts(needs: VoiceNeeds): string {
    const parts: string[] = [];
    if (needs.downloadMb) parts.push(`${needs.kind === 'file' ? 'скачать голос' : 'скачать'} ~${sizeText(needs.downloadMb)}`);
    if (needs.diskMb && needs.kind === 'engine') parts.push(`на диске ~${sizeText(needs.diskMb)}`);
    if (needs.minutes) parts.push(`около ${needs.minutes} ${plural(needs.minutes, 'минуты', 'минут', 'минут')}`);
    if (parts.length === 0) return needs.kind === 'file' ? 'Голос скачается с сайта Piper или Silero.' : 'Модель скачается и встанет на этот компьютер.';
    const text = parts.join(', ');
    return `${text[0].toUpperCase()}${text.slice(1)}.`;
}

export type NeedsBusy = 'install' | 'fallback' | 'reimprint' | null;

export interface NeedsProps {
    needs: VoiceNeeds;
    /** Запасной голос: им говорить, пока модели нет. */
    fallback?: string | null;
    busy?: NeedsBusy;
    error?: string | null;
    /** Памяти впритык: текст шлюза, следующее «Скачать и включить» — всё равно. */
    tight?: string | null;
    onInstall: () => void;
    onFallback?: () => void;
    /** Свой голос другой модели: переснять из записей под ту, что стоит. */
    onReimprint?: (engine: string) => void;
}

/** Содержимое окна: что нужно, сколько, и что можно сделать. Мастер своего голоса рисует его у себя, без окна. */
export function NeedsBody({ needs, fallback, busy = null, error, tight, onInstall, onFallback, onReimprint }: NeedsProps) {
    const alternatives = onReimprint ? (needs.alternatives ?? []).filter((entry) => entry.kind === 'reimprint') : [];
    const waiting = busy !== null;
    const spin = (what: NeedsBusy) => busy === what && <span class="spinner spinner-sm" aria-hidden="true" />;
    return (
        <>
            <p class="needs-facts">{facts(needs)}</p>
            <p class="t-caption m-0">
                {needs.kind === 'file' ? 'Голос включится сам, когда файл скачается.' : 'Голос включится сам, когда модель встанет.'}
                {fallback ? ` Пока — говорит «${fallback}».` : ''}
            </p>
            {tight && (
                <p class="t-caption m-0 c-pill-yandex-fg" role="status">
                    {tight}
                </p>
            )}
            {error && (
                <p class="t-caption m-0 c-dot-offline" role="alert">
                    {error}
                </p>
            )}
            <div class="needs-actions">
                <button type="button" class="pill pill-own pill-btn inline-flex items-center gap-6" disabled={waiting} onClick={onInstall}>
                    {spin('install')}
                    {tight ? 'Включить всё равно' : 'Скачать и включить'}
                </button>
                {alternatives.map((entry) => (
                    <button
                        key={entry.engine}
                        type="button"
                        class="pill pill-off pill-btn inline-flex items-center gap-6"
                        disabled={waiting}
                        onClick={() => onReimprint?.(entry.engine)}
                    >
                        {spin('reimprint')}
                        Переснять для {entry.title ?? entry.engine} из записей
                    </button>
                ))}
                {fallback && onFallback && (
                    <button type="button" class="pill pill-off pill-btn inline-flex items-center gap-6" disabled={waiting} onClick={onFallback}>
                        {spin('fallback')}
                        Пока говорить голосом «{fallback}»
                    </button>
                )}
            </div>
        </>
    );
}

/** Окно «нужна модель» поверх экрана: о ком речь — в подзаголовке. */
export function NeedsDialog({ who, onClose, ...props }: NeedsProps & { who: string; onClose: () => void }) {
    const what = needsWhat(props.needs);
    return (
        <Sheet
            title={`${props.needs.kind === 'file' ? 'Нужен' : 'Нужна'} ${what}`}
            about={who}
            onClose={onClose}
            footer={
                <button type="button" class="pill pill-off pill-btn justify-center min-h-44" onClick={onClose}>
                    Не сейчас
                </button>
            }
        >
            <NeedsBody {...props} />
        </Sheet>
    );
}

/** О каком голосе спросить «нужна модель» и как повторить выбор. */
export interface NeedsAsk {
    /** Кто: «Артур», «Мой голос». */
    who: string;
    needs: VoiceNeeds;
    /** Запасной голос, которым говорить, пока модели нет. */
    fallback?: string | null;
    /** Свой голос (id в лаборатории): у него может быть «переснять из записей». */
    ownId?: string;
    /** Тот же выбор ещё раз: с install — поставить модель и включить сам; с anyway — при памяти впритык. */
    retry: (options: { install: boolean; anyway: boolean }) => Promise<unknown>;
    /** «Пока говорить голосом …»: что сделать (выбрать без модели); нет — просто закрыть окно. */
    onFallback?: () => Promise<unknown> | void;
}

/**
 * Ход «нужна модель»: открыть окно, «Скачать и включить» (повтор с install), «всё равно» при памяти впритык,
 * «переснять из записей», запасной голос. dialog — окно поверх экрана, body — то же внутри своего окна (мастер).
 */
export function useNeedsFlow(onDone: (message: string) => void) {
    const [ask, setAsk] = useState<NeedsAsk | null>(null);
    const [busy, setBusy] = useState<NeedsBusy>(null);
    const [error, setError] = useState<string | null>(null);
    const [tight, setTight] = useState<string | null>(null);

    const open = (next: NeedsAsk): void => {
        setAsk(next);
        setError(null);
        setTight(null);
    };
    const close = (): void => setAsk(null);
    const run = async (what: NeedsBusy, action: () => Promise<unknown> | void, done: string): Promise<void> => {
        setBusy(what);
        setError(null);
        try {
            await action();
            setAsk(null);
            setTight(null);
            onDone(done);
        } catch (e) {
            if (what === 'install' && isTight(e)) setTight(problemOf(e));
            else setError(problemOf(e));
        } finally {
            setBusy(null);
        }
    };

    const props: NeedsProps | null = ask && {
        needs: ask.needs,
        fallback: ask.fallback,
        busy,
        error,
        tight,
        onInstall: () =>
            void run(
                'install',
                () => ask.retry({ install: true, anyway: tight !== null }),
                tight !== null
                    ? 'Включаю всё равно.'
                    : ask.needs.kind === 'file'
                      ? 'Голос скачивается — включится сам.'
                      : 'Модель ставится — голос включится сам, когда она встанет.',
            ),
        onFallback: ask.fallback ? () => void run('fallback', () => ask.onFallback?.(), `Пока говорит «${ask.fallback}».`) : undefined,
        onReimprint: ask.ownId
            ? (engine: string) =>
                  void run('reimprint', () => api.reimprintVoice(ask.ownId!, engine), 'Переснимаю голос из записей — около минуты. Потом выберите его снова.')
            : undefined,
    };
    return {
        open,
        close,
        isOpen: ask !== null,
        dialog: props && ask ? <NeedsDialog who={ask.who} onClose={close} {...props} /> : null,
        body: props ? <NeedsBody {...props} /> : null,
    };
}
