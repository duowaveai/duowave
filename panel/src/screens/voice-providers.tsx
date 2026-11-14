// «Облачные голоса» во вкладке «Голос» (gateway/src/capabilities/voices.rs): поставщики, что озвучивают реплики
// приложений помимо компьютера, — ElevenLabs по ключу владельца. Ключ хранится в сейфе шлюза и вставляется здесь,
// руками; приложение его не видит и само в ElevenLabs не ходит. Облачным голосом приложение говорит только после
// «да» владельца на «Облачные голоса» (лист «Разрешить всё» или вопрос шины).

import { useEffect, useState } from 'preact/hooks';

import { VoiceProviderCard, VoiceProviderCheck, api, can, problemOf } from '../api';
import { formatNumber, tr } from '../i18n';
import { Action, Card, CardNote } from '../ui/cards';
import { ask } from '../ui/confirm';

/** Где взять ключ — страница самого поставщика. */
const KEY_PAGES: Record<string, string> = { elevenlabs: 'https://elevenlabs.io/app/settings/api-keys' };

function balanceLine(check: VoiceProviderCheck): string | null {
    const balance = check.balance;
    if (!balance || typeof balance.limit !== 'number') return null;
    const left = Math.max(0, balance.limit - (balance.used ?? 0));
    return tr.cloudVoices.left(formatNumber(left), formatNumber(balance.limit), balance.unit ?? '', balance.tier ?? null);
}

export function VoiceProviders() {
    const [providers, setProviders] = useState<VoiceProviderCard[] | null>(null);
    const [draft, setDraft] = useState<Record<string, string>>({});
    const [busy, setBusy] = useState<Record<string, boolean>>({});
    const [notes, setNotes] = useState<Record<string, CardNote>>({});
    const manage = can('connectors.manage');

    const load = (): void => {
        api.voiceProviders()
            .then((answer) => setProviders(answer.providers.filter((provider) => provider.cloud || provider.paid)))
            .catch(() => setProviders([]));
    };
    useEffect(load, []);

    const say = (id: string, note: CardNote): void => setNotes((all) => ({ ...all, [id]: note }));
    const mark = (id: string, on: boolean): void => setBusy((all) => ({ ...all, [id]: on }));

    const check = async (provider: VoiceProviderCard): Promise<void> => {
        mark(provider.id, true);
        try {
            const answer = await api.checkVoiceProvider(provider.id);
            if (answer.problem) say(provider.id, { error: answer.problem });
            else say(provider.id, { notice: [tr.cloudVoices.works(answer.voices), balanceLine(answer)].filter(Boolean).join('. ') });
        } catch (error) {
            say(provider.id, { error: problemOf(error) });
        } finally {
            mark(provider.id, false);
        }
    };

    const save = async (provider: VoiceProviderCard): Promise<void> => {
        const key = (draft[provider.id] ?? '').trim();
        if (!key) return;
        mark(provider.id, true);
        try {
            await api.setVoiceKey(provider.id, key);
            setDraft((all) => ({ ...all, [provider.id]: '' }));
            load();
        } catch (error) {
            say(provider.id, { error: problemOf(error) });
            mark(provider.id, false);
            return;
        }
        mark(provider.id, false);
        await check(provider);
    };

    const remove = async (provider: VoiceProviderCard): Promise<void> => {
        if (!(await ask({ title: tr.cloudVoices.removeTitle(provider.title), text: tr.cloudVoices.removeText, ok: tr.cloudVoices.remove, danger: true }))) return;
        mark(provider.id, true);
        try {
            await api.removeVoiceKey(provider.id);
            say(provider.id, { notice: tr.cloudVoices.removed });
            load();
        } catch (error) {
            say(provider.id, { error: problemOf(error) });
        } finally {
            mark(provider.id, false);
        }
    };

    if (!providers?.length) return null;
    return (
        <section class="card glass card-list" aria-label={tr.cloudVoices.title}>
            <div class="card-head">
                <h2 class="t-title-xs m-0">{tr.cloudVoices.title}</h2>
                <span class="t-meta">{tr.cloudVoices.forApps}</span>
            </div>
            <p class="t-caption m-0 mb-6">{tr.cloudVoices.intro}</p>
            {providers.map((provider) => {
                const keySet = !!provider.key?.set;
                return (
                    <Card
                        key={provider.id}
                        title={provider.title}
                        description={provider.about}
                        meta={keySet ? tr.cloudVoices.keySet((provider.key?.hint ?? '').replace(/^…/, '')) : tr.cloudVoices.noKey}
                        pills={[
                            { text: tr.cloudVoices.cloud, cls: 'pill-yandex' },
                            ...(provider.paid ? [{ text: tr.cloudVoices.paid, cls: 'pill-yandex' }] : []),
                            ...(provider.ready ? [{ text: tr.cloudVoices.connected, cls: 'pill-own' }] : []),
                        ]}
                        note={notes[provider.id] ?? (provider.problem ? { warning: provider.problem } : undefined)}
                        action={keySet && manage ? <Action label={tr.cloudVoices.check} busy={busy[provider.id]} onClick={() => void check(provider)} /> : null}
                    >
                        {manage ? (
                            <form
                                class="flex gap-8 flex-wrap items-center"
                                onSubmit={(event) => {
                                    event.preventDefault();
                                    void save(provider);
                                }}
                            >
                                <input
                                    class="field flex-1-1-200 min-w-0"
                                    type="password"
                                    autocomplete="off"
                                    aria-label={tr.cloudVoices.keyOf(provider.title)}
                                    placeholder={keySet ? tr.cloudVoices.newKey : tr.cloudVoices.keyOf(provider.title)}
                                    value={draft[provider.id] ?? ''}
                                    onInput={(event) => {
                                        const value = (event.target as HTMLInputElement).value;
                                        setDraft((all) => ({ ...all, [provider.id]: value }));
                                    }}
                                />
                                <Action
                                    label={tr.cloudVoices.save}
                                    tone="on"
                                    busy={busy[provider.id]}
                                    disabled={!(draft[provider.id] ?? '').trim()}
                                    onClick={() => void save(provider)}
                                />
                                {keySet && <Action label={tr.cloudVoices.removeKey} onClick={() => void remove(provider)} />}
                                {KEY_PAGES[provider.id] && (
                                    <a class="t-caption c-accent-text" href={KEY_PAGES[provider.id]} target="_blank" rel="noopener noreferrer">
                                        {tr.cloudVoices.whereKey}
                                    </a>
                                )}
                            </form>
                        ) : (
                            <span class="t-caption">{tr.cloudVoices.keyByPeople}</span>
                        )}
                    </Card>
                );
            })}
        </section>
    );
}
