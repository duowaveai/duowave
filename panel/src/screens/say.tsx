// «Сказать» (владелец, 20.11): своё слово вписать и услышать — голосом персонажа или Алисой. Звучит на колонке, а без
// живой колонки — на этом компьютере. Одно поле: в «Настройки → Голос» и окном из поиска («Сказать текст»).
// - Колонка: `/station/sessions/latest/say` с `voice` — шлюз говорит её сессией своим голосом или синтезом Яндекса;
//   какого голоса нет — отвечает словами (409) до того, как колонка заговорит.
// - Компьютер: свой голос дома (`/api/v1/talk/speech`) в динамики. Алиса говорит только через колонку.

import { useEffect, useState } from 'preact/hooks';

import { ApiError, Health, SessionSnapshot, UnsupportedEntry, api, can, problemOf } from '../api';
import { IconSend } from '../icons';
import { Slot } from '../shell/slot';
import { tr } from '../i18n';
import { Sheet } from '../ui/sheet';
import { sayAloud } from '../voice/speak';

interface Props {
    health: Health | null;
    debug: { sessions: SessionSnapshot[] | null; unsupported: UnsupportedEntry[]; error: unknown };
    onClose: () => void;
}

type Voice = 'own' | 'alice';

type Outcome = { state: 'idle' } | { state: 'sending' } | { state: 'done'; where: 'station' | 'computer' } | { state: 'error'; message: string };

const VOICE_KEY = 'glass-say-voice';

function rememberedVoice(): Voice {
    try {
        return localStorage.getItem(VOICE_KEY) === 'alice' ? 'alice' : 'own';
    } catch {
        return 'own';
    }
}

/** Сессии колонок: свои, если их не дали, — раз при открытии. */
function useSessions(given?: SessionSnapshot[] | null): SessionSnapshot[] | null {
    const [own, setOwn] = useState<SessionSnapshot[] | null>(null);
    useEffect(() => {
        if (given !== undefined || !can('station.view')) return;
        let alive = true;
        api.sessions()
            .then((sessions) => alive && setOwn(sessions))
            .catch(() => alive && setOwn([]));
        return () => {
            alive = false;
        };
    }, [given]);
    return given !== undefined ? given : own;
}

/** Поле «Сказать»: текст, чей голос, кнопка. Где прозвучит — колонка, иначе этот компьютер. */
export function SayField({ sessions: given, autoFocus = false }: { sessions?: SessionSnapshot[] | null; autoFocus?: boolean }) {
    const t = tr.say;
    const sessions = useSessions(given);
    const [text, setText] = useState('');
    const [voice, setVoice] = useState<Voice>(rememberedVoice);
    const [outcome, setOutcome] = useState<Outcome>({ state: 'idle' });
    const live = sessions?.find((session) => !session.closedAt && session.canSpeak);
    const where: 'station' | 'computer' = live && can('station.say') ? 'station' : 'computer';
    // Алиса — только голосом колонки: на компьютере её синтеза нет.
    const aliceHere = where === 'station';
    const chosen: Voice = voice === 'alice' && !aliceHere ? 'own' : voice;

    const pick = (next: Voice): void => {
        setVoice(next);
        setOutcome({ state: 'idle' });
        try {
            localStorage.setItem(VOICE_KEY, next);
        } catch {
            /* без памяти — выбор только на это окно */
        }
    };

    const send = async (): Promise<void> => {
        const value = text.trim();
        if (!value || outcome.state === 'sending') return;
        setOutcome({ state: 'sending' });
        if (where === 'computer') {
            const said = await sayAloud(value);
            setOutcome(said ? { state: 'done', where } : { state: 'error', message: t.noOwnVoiceHere });
            return;
        }
        try {
            const { outcome: said } = await api.say(value, chosen);
            if (said === 'failed') {
                setOutcome({ state: 'error', message: t.noAnswer });
                return;
            }
            setOutcome({ state: 'done', where });
        } catch (e) {
            const status = e instanceof ApiError ? e.status : 0;
            setOutcome({
                state: 'error',
                message: status === 404 ? t.noSession : status === 403 || status === 409 ? problemOf(e) : t.failed,
            });
        }
    };

    return (
        <div class="flex flex-col gap-10">
            <textarea
                value={text}
                rows={3}
                maxLength={1000}
                onInput={(e) => {
                    setText((e.target as HTMLTextAreaElement).value);
                    if (outcome.state !== 'sending') setOutcome({ state: 'idle' });
                }}
                onKeyDown={(e) => {
                    // Enter — сказать, Shift+Enter — новая строка.
                    if (e.key === 'Enter' && !e.shiftKey) {
                        e.preventDefault();
                        void send();
                    }
                }}
                placeholder={t.placeholder}
                aria-label={t.inputLabel}
                autoFocus={autoFocus}
                class="say-text py-10 px-14 r-12 border-glass bg-icon-bg outline-none"
            />
            <div class="flex gap-8 items-center flex-wrap">
                <div class="seg" role="group" aria-label={t.voiceLabel}>
                    <button type="button" aria-pressed={chosen === 'own'} onClick={() => pick('own')}>
                        {t.voiceOwn}
                    </button>
                    <button type="button" aria-pressed={chosen === 'alice'} disabled={!aliceHere} title={aliceHere ? undefined : t.aliceOnStation} onClick={() => pick('alice')}>
                        {t.voiceAlice}
                    </button>
                </div>
                <span class="w-cap flex-1 min-w-0">{where === 'station' ? t.onStation : t.onComputer}</span>
                <button class="btn btn-main flex-none" onClick={() => void send()} disabled={!text.trim() || outcome.state === 'sending'}>
                    <IconSend size={16} />
                    {outcome.state === 'sending' ? t.saying : t.sendLabel}
                </button>
            </div>
            {outcome.state === 'done' && (
                <p class="t-meta m-0" role="status">
                    {outcome.where === 'station' ? t.said : t.saidHere}
                </p>
            )}
            {outcome.state === 'error' && (
                <p class="t-meta m-0 c-dot-offline" role="alert">
                    {outcome.message}
                </p>
            )}
        </div>
    );
}

/** Окно «Сказать» из поиска. */
export function SaySheet({ health, debug, onClose }: Props) {
    const live = debug.sessions?.find((s) => !s.closedAt);
    const speakable = live?.canSpeak === true;
    return (
        <Sheet
            title={tr.say.title}
            meta={<span class={`pill ${health?.station?.connected ? 'pill-own' : 'pill-off'}`}>{health?.station?.connected ? tr.say.stationOnline : tr.say.stationOffline}</span>}
            onClose={onClose}
        >
            <SayField sessions={debug.sessions} autoFocus />
            <Slot name="say.extras" props={{ speakable }} />
        </Sheet>
    );
}
