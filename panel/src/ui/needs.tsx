// «Нужна модель»: голос сам знает, чего ему не хватает (docs/plans/characters.md, раздел 5). Одно окно для всех —
// персонажи, свои голоса, Piper и Silero: сколько скачать, сколько займёт, сколько ждать, «Скачать и включить» и
// «Пока говорить голосом …». Тут же — метки состояния голоса для карточек и плиток.

import type { ComponentChildren } from 'preact';
import { useRef, useState } from 'preact/hooks';

import type { CharacterVoice, HomeVoicePlan, TtsCard, VoiceNeeds } from '../api';
import { api, isTight, problemOf } from '../api';
import { Sheet } from './sheet';
import { EnginePicker, defaultEngine, fitsDisk, usableEngines } from './voice-plan';
import { formatNumber, tr } from '../i18n';

/** «60 МБ», «3,1 ГБ», «10 ГБ». */
export function sizeText(mb: number): string {
    if (mb < 1024) return tr.ui.sizeMb(Math.max(1, Math.round(mb)));
    const gb = mb / 1024;
    return tr.ui.sizeGb(formatNumber(gb >= 10 ? Math.round(gb) : Math.round(gb * 10) / 10));
}

/** Сколько займёт на диске (а не известно — сколько скачать). */
export function needsSize(needs?: VoiceNeeds | null): string | null {
    const mb = needs?.diskMb ?? needs?.downloadMb;
    return mb ? sizeText(mb) : null;
}

/** «модель OmniVoice», «файл голоса». */
export function needsWhat(needs: VoiceNeeds): string {
    return needs.kind === 'engine' ? tr.ui.needsModel(needs.title ?? needs.id) : tr.ui.needsVoiceFile;
}

/** Метка готовности голоса: нужна модель, качается, не пойдёт здесь. Готовому голосу метка не нужна. */
export function voicePill(voice: Pick<TtsCard, 'state' | 'progress' | 'needs'> | CharacterVoice | null | undefined): { text: string; cls: string } | null {
    if (!voice?.state) return null;
    if (voice.state === 'needs') {
        const size = needsSize(voice.needs);
        const what = voice.needs?.kind === 'file' ? tr.ui.pillNeedsFile : tr.ui.pillNeedsModel;
        return { text: size ? `${what} · ${size}` : what, cls: 'pill-yandex' };
    }
    if (voice.state === 'installing')
        return { text: voice.progress !== undefined ? tr.ui.pillDownloadingPct(Math.round(voice.progress)) : tr.ui.pillDownloading, cls: 'pill-off' };
    if (voice.state === 'unsupported') return { text: tr.ui.pillUnsupported, cls: 'pill-off' };
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
    if (needs.downloadMb) parts.push(tr.ui.factDownload(needs.kind === 'file', sizeText(needs.downloadMb)));
    if (needs.diskMb && needs.kind === 'engine') parts.push(tr.ui.factDisk(sizeText(needs.diskMb)));
    if (needs.minutes) parts.push(tr.ui.factMinutes(needs.minutes));
    if (parts.length === 0) return needs.kind === 'file' ? tr.ui.factsFileDefault : tr.ui.factsModelDefault;
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
    /** Выбор модели на компьютере-Турбо (п. 40) — под текстом, до кнопок. */
    picker?: ComponentChildren;
    /** Ставить нечего: ни одна модель не помещается на диск компьютера — «Скачать и включить» не нажать. */
    blocked?: boolean;
}

/** «Скачать и включить» (памяти впритык — «Включить всё равно»): главное действие; `main` — лайм, в нижней полосе окна. */
function InstallButton({ busy = null, tight, onInstall, main, blocked }: Pick<NeedsProps, 'busy' | 'tight' | 'onInstall' | 'blocked'> & { main: boolean }) {
    return (
        <button type="button" class={`btn ${main ? 'btn-main' : 'btn-plain'}`} disabled={busy !== null || !!blocked} aria-busy={busy === 'install' || undefined} onClick={onInstall}>
            {busy === 'install' && <span class="spinner spinner-sm" aria-hidden="true" />}
            {tight ? tr.ui.enableAnyway : tr.ui.downloadAndEnable}
        </button>
    );
}

/**
 * Содержимое окна: что нужно, сколько, и что можно сделать. Мастер своего голоса рисует его у себя, внутри своего
 * окна (`inline`: и «Скачать и включить» здесь же); окно «нужна модель» ставит его в нижнюю полосу.
 */
export function NeedsBody({ needs, fallback, busy = null, error, tight, onInstall, onFallback, onReimprint, picker, blocked, inline = true }: NeedsProps & { inline?: boolean }) {
    const alternatives = onReimprint ? (needs.alternatives ?? []).filter((entry) => entry.kind === 'reimprint') : [];
    const waiting = busy !== null;
    const spin = (what: NeedsBusy) => busy === what && <span class="spinner spinner-sm" aria-hidden="true" />;
    const others = alternatives.length > 0 || !!(fallback && onFallback);
    return (
        <>
            <p class="win-lead">{facts(needs)}</p>
            <p class="win-text m-0">
                {needs.kind === 'file' ? tr.ui.willEnableFile : tr.ui.willEnableModel}
                {fallback ? ` ${tr.ui.meanwhileSpeaks(fallback)}` : ''}
            </p>
            {picker}
            {tight && (
                <p class="win-callout m-0" role="status">
                    {tight}
                </p>
            )}
            {error && (
                <p class="win-callout m-0" role="alert">
                    {error}
                </p>
            )}
            {(inline || others) && (
                <div class="flex flex-wrap gap-8">
                    {inline && <InstallButton busy={busy} tight={tight} onInstall={onInstall} main={false} blocked={blocked} />}
                    {alternatives.map((entry) => (
                        <button
                            key={entry.engine}
                            type="button"
                            class="btn btn-plain"
                            disabled={waiting}
                            aria-busy={busy === 'reimprint' || undefined}
                            onClick={() => onReimprint?.(entry.engine)}
                        >
                            {spin('reimprint')}
                            {tr.ui.reimprintFor(entry.title ?? entry.engine)}
                        </button>
                    ))}
                    {fallback && onFallback && (
                        <button type="button" class="btn btn-plain" disabled={waiting} aria-busy={busy === 'fallback' || undefined} onClick={onFallback}>
                            {spin('fallback')}
                            {tr.ui.speakWithFallback(fallback)}
                        </button>
                    )}
                </div>
            )}
        </>
    );
}

/** Окно «нужна модель» поверх экрана: о ком речь — меткой в шапке; «Скачать и включить» — внизу справа. */
export function NeedsDialog({ who, onClose, ...props }: NeedsProps & { who: string; onClose: () => void }) {
    const what = needsWhat(props.needs);
    return (
        <Sheet
            title={tr.ui.needsTitle(props.needs.kind === 'file', what)}
            meta={<span class="win-chip">{who}</span>}
            onClose={onClose}
            footer={
                <>
                    <button type="button" class="btn btn-plain" onClick={onClose}>
                        {tr.ui.notNow}
                    </button>
                    <InstallButton busy={props.busy} tight={props.tight} onInstall={props.onInstall} main blocked={props.blocked} />
                </>
            }
        >
            <NeedsBody {...props} inline={false} />
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
    /**
     * Голос персонажа на компьютере-Турбо (п. 40): центр спрашивает у Турбо план, окно показывает тот же выбор модели,
     * что мастер установки; «Скачать и включить» сначала говорит центру выбранную модель. Центр старый или Турбо нет —
     * окно как раньше.
     */
    homeVoice?: { character?: string; package?: string };
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
    const [plan, setPlan] = useState<HomeVoicePlan | null>(null);
    const [engine, setEngine] = useState<string | null>(null);
    /** Выбор модели — в ref: retry окна собран при открытии, а модель меняют потом. */
    const chosen = useRef<string | null>(null);
    const pick = (next: string | null): void => {
        chosen.current = next;
        setEngine(next);
    };

    const open = (next: NeedsAsk): void => {
        setAsk(next);
        setError(null);
        setTight(null);
        setPlan(null);
        pick(null);
        if (next.homeVoice)
            void api
                .homeVoicePlan(next.homeVoice)
                .then((fresh) => {
                    setPlan(fresh.plan ? fresh : null);
                    pick(fresh.plan ? defaultEngine(fresh) : null);
                })
                .catch(() => undefined);
    };
    /** Выбранная модель — центру, до самой установки (он помнит и передаст Турбо, и вернувшемуся тоже). */
    const tellEngine = async (): Promise<void> => {
        if (ask?.homeVoice && plan?.plan && chosen.current) await api.setHomeVoiceEngine(ask.homeVoice, chosen.current);
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

    // Выбрали другую модель Турбо — окно говорит о ней: её имя в шапке, её размер в строке «Скачать …».
    const picked = usableEngines(plan).find((entry) => entry.id === engine);
    const needs: VoiceNeeds | null =
        ask && picked && ask.needs.kind === 'engine'
            ? { ...ask.needs, id: picked.id, title: picked.title, downloadMb: picked.state === 'ready' ? undefined : (picked.totalMb ?? picked.diskMb), diskMb: picked.diskMb, minutes: undefined }
            : (ask?.needs ?? null);
    const props: NeedsProps | null = ask && needs && {
        needs,
        fallback: ask.fallback,
        busy,
        error,
        tight,
        onInstall: () =>
            void run(
                'install',
                async () => {
                    await tellEngine();
                    return ask.retry({ install: true, anyway: tight !== null });
                },
                tight !== null
                    ? tr.ui.enablingAnyway
                    : ask.needs.kind === 'file'
                      ? tr.ui.voiceDownloading
                      : tr.ui.modelInstalling,
            ),
        onFallback: ask.fallback ? () => void run('fallback', () => ask.onFallback?.(), tr.ui.meanwhileSpeaks(ask.fallback ?? '')) : undefined,
        onReimprint: ask.ownId
            ? (engine: string) =>
                  void run('reimprint', () => api.reimprintVoice(ask.ownId!, engine), tr.ui.reimprinting)
            : undefined,
        picker: plan ? <EnginePicker plan={plan} engine={engine} onSelect={pick} /> : undefined,
        blocked: !!plan && !(picked && fitsDisk(picked)),
    };
    return {
        open,
        close,
        isOpen: ask !== null,
        dialog: props && ask ? <NeedsDialog who={ask.who} onClose={close} {...props} /> : null,
        body: props ? <NeedsBody {...props} /> : null,
    };
}
