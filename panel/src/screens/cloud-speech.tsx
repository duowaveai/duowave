// Облачный слух и голос колонки (gateway/src/speech/cloud_speech.rs, docs/plans/speaker-alone.md, этап 1): колонка
// слышит и говорит через облако — без компьютера и без Яндекса. Ключ OpenAI и Groq — общий с «Мозгом»: уже вставленный
// там подходит и здесь. Ключ хранится на колонке, в сейфе шлюза. Изменение работает со следующей фразы, без перезапуска.

import { useEffect, useState } from 'preact/hooks';

import { CloudPart, CloudPartView, CloudService, CloudSpeechView, api, can, problemOf } from '../api';
import { tr } from '../i18n';
import { Action, CardNote } from '../ui/cards';

const KEY_PAGES: Partial<Record<CloudService, string>> = {
    openai: 'https://platform.openai.com/api-keys',
    groq: 'https://console.groq.com/keys',
};

const SERVICE_NAMES: Record<CloudService, string> = { openai: 'OpenAI', groq: 'Groq', custom: '', off: '' };

interface Draft {
    service: CloudService;
    model: string;
    voice: string;
    url: string;
    key: string;
}

const draftOf = (part: CloudPartView): Draft => ({
    service: part.service,
    model: part.model ?? '',
    voice: part.voice ?? '',
    url: part.url ?? '',
    key: '',
});

/** Одна часть — слух или голос. `onChanged` — шлюз принял изменение (окно «Голос» перечитывает выбор). */
export function CloudSpeechCard({ part, onChanged }: { part: CloudPart; onChanged?: () => void }) {
    const [view, setView] = useState<CloudSpeechView | null>(null);
    const [draft, setDraft] = useState<Draft | null>(null);
    const [busy, setBusy] = useState<'save' | 'check' | null>(null);
    const [note, setNote] = useState<CardNote>({});
    const manage = can('models.manage');

    const apply = (fresh: CloudSpeechView): void => {
        setView(fresh);
        setDraft(draftOf(fresh[part]));
    };

    useEffect(() => {
        api.cloudSpeech()
            .then(apply)
            .catch(() => setView(null));
    }, [part]);

    if (!view || !view.active || !draft) return null;
    const current = view[part];
    const hearing = part === 'hearing';
    const services: CloudService[] = hearing ? ['openai', 'groq', 'custom', 'off'] : ['openai', 'custom', 'off'];
    const set = (patch: Partial<Draft>): void => setDraft({ ...draft, ...patch });
    const sameService = draft.service === current.service && !current.fromEnv;
    const keyKnown = sameService && current.keySet;
    const shared = draft.service === 'openai' || draft.service === 'groq';
    const defaultModel =
        draft.service === 'groq'
            ? view.defaults.groqHearingModel
            : draft.service === 'openai'
              ? hearing
                  ? view.defaults.openaiHearingModel
                  : view.defaults.openaiVoiceModel
              : '';

    const save = async (): Promise<void> => {
        setBusy('save');
        setNote({});
        try {
            const fresh = await api.setCloudSpeech({
                part,
                service: draft.service,
                model: draft.model.trim() || undefined,
                voice: hearing ? undefined : draft.voice.trim() || undefined,
                url: draft.service === 'custom' ? draft.url.trim() : undefined,
                key: draft.key.trim() || undefined,
            });
            apply(fresh);
            setNote({ notice: tr.cloudSpeech.saved });
            onChanged?.();
        } catch (error) {
            setNote({ error: problemOf(error) });
        } finally {
            setBusy(null);
        }
    };

    const check = async (): Promise<void> => {
        setBusy('check');
        setNote({});
        try {
            const answer = await api.checkCloudSpeech(part);
            setNote(answer.problem ? { error: answer.problem } : { notice: tr.cloudSpeech.works });
        } catch (error) {
            setNote({ error: problemOf(error) });
        } finally {
            setBusy(null);
        }
    };

    const title = hearing ? tr.cloudSpeech.hearingTitle : tr.cloudSpeech.voiceTitle;
    const serviceLabel = (service: CloudService): string => tr.cloudSpeech[service];
    const keyPage = KEY_PAGES[draft.service];

    return (
        <section class="card glass py-12 px-16 mb-10" aria-label={title}>
            <h2 class="t-title-xs m-0 mb-6 flex items-center gap-8">
                {title}
                <span class={`pill ${current.ready ? 'pill-own' : 'pill-off'}`}>
                    {current.ready ? tr.cloudSpeech.ready : tr.cloudSpeech.notReady}
                </span>
            </h2>
            <p class="t-body m-0 mb-10">{hearing ? tr.cloudSpeech.hearingAbout : tr.cloudSpeech.voiceAbout}</p>
            {current.fromEnv && <p class="t-caption m-0 mb-8">{tr.cloudSpeech.fromEnv}</p>}
            {!manage ? (
                <p class="t-caption m-0">{tr.cloudSpeech.adminOnly}</p>
            ) : (
                <form
                    class="flex flex-col gap-8"
                    onSubmit={(event) => {
                        event.preventDefault();
                        void save();
                    }}
                >
                    <label class="list-row">
                        <span class="flex-1 min-w-180">
                            <b>{tr.cloudSpeech.service}</b>
                        </span>
                        <select
                            class="field max-w-full min-w-0 flex-1-1-220"
                            value={draft.service}
                            onChange={(e) =>
                                set({ service: (e.target as HTMLSelectElement).value as CloudService, model: '', key: '' })
                            }
                        >
                            {services.map((service) => (
                                <option key={service} value={service}>
                                    {serviceLabel(service)}
                                </option>
                            ))}
                        </select>
                    </label>
                    {draft.service === 'custom' && (
                        <label class="list-row">
                            <span class="flex-1 min-w-180">
                                <b>{tr.cloudSpeech.url}</b>
                            </span>
                            <input
                                class="field flex-1-1-220 min-w-0"
                                type="url"
                                placeholder={tr.cloudSpeech.urlHint}
                                value={draft.url}
                                onInput={(e) => set({ url: (e.target as HTMLInputElement).value })}
                            />
                        </label>
                    )}
                    {draft.service !== 'off' && (
                        <label class="list-row">
                            <span class="flex-1 min-w-180">
                                <b>{tr.cloudSpeech.model}</b>
                            </span>
                            <input
                                class="field flex-1-1-220 min-w-0"
                                type="text"
                                placeholder={defaultModel}
                                value={draft.model}
                                onInput={(e) => set({ model: (e.target as HTMLInputElement).value })}
                            />
                        </label>
                    )}
                    {!hearing && draft.service !== 'off' && (
                        <label class="list-row">
                            <span class="flex-1 min-w-180">
                                <b>{tr.cloudSpeech.voice}</b>
                            </span>
                            {draft.service === 'openai' ? (
                                <select
                                    class="field max-w-full min-w-0 flex-1-1-220"
                                    value={draft.voice || view.defaults.openaiVoice}
                                    onChange={(e) => set({ voice: (e.target as HTMLSelectElement).value })}
                                >
                                    {view.defaults.openaiVoices.map((voice) => (
                                        <option key={voice} value={voice}>
                                            {voice}
                                        </option>
                                    ))}
                                </select>
                            ) : (
                                <input
                                    class="field flex-1-1-220 min-w-0"
                                    type="text"
                                    value={draft.voice}
                                    onInput={(e) => set({ voice: (e.target as HTMLInputElement).value })}
                                />
                            )}
                        </label>
                    )}
                    {draft.service !== 'off' && (
                        <div class="flex flex-col gap-4">
                            <input
                                class="field min-w-0"
                                type="password"
                                autocomplete="off"
                                aria-label={tr.cloudSpeech.keyOf(SERVICE_NAMES[draft.service] || tr.cloudSpeech.custom)}
                                placeholder={
                                    keyKnown
                                        ? tr.cloudSpeech.newKey
                                        : draft.service === 'custom'
                                          ? tr.cloudSpeech.keyOptional
                                          : tr.cloudSpeech.keyOf(SERVICE_NAMES[draft.service])
                                }
                                value={draft.key}
                                onInput={(e) => set({ key: (e.target as HTMLInputElement).value })}
                            />
                            <span class="t-caption">
                                {keyKnown && shared
                                    ? tr.cloudSpeech.keyShared(SERVICE_NAMES[draft.service])
                                    : keyKnown
                                      ? tr.cloudSpeech.keyOwn
                                      : shared
                                        ? tr.cloudSpeech.keySharedNote
                                        : ''}{' '}
                                {tr.cloudSpeech.onStation}{' '}
                                {keyPage && (
                                    <a class="c-accent-text" href={keyPage} target="_blank" rel="noopener noreferrer">
                                        {tr.cloudSpeech.whereKey}
                                    </a>
                                )}
                            </span>
                        </div>
                    )}
                    <div class="flex flex-wrap gap-8">
                        <Action label={tr.cloudSpeech.save} tone="on" busy={busy === 'save'} disabled={!!busy} onClick={() => void save()} />
                        {current.ready && (
                            <Action label={tr.cloudSpeech.check} busy={busy === 'check'} disabled={!!busy} onClick={() => void check()} />
                        )}
                    </div>
                    {note.notice && <span class="t-caption c-accent-text">{note.notice}</span>}
                    {note.error && (
                        <span class="t-caption c-dot-offline" role="alert">
                            {note.error}
                        </span>
                    )}
                </form>
            )}
        </section>
    );
}
