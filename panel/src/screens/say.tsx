import { useState } from 'preact/hooks';

import { ApiError, Health, SessionSnapshot, UnsupportedEntry, api, problemOf } from '../api';
import { IconSend } from '../icons';
import { Slot } from '../shell/slot';

interface Props {
    health: Health | null;
    debug: { sessions: SessionSnapshot[] | null; unsupported: UnsupportedEntry[]; error: unknown };
    onClose: () => void;
}

type Outcome = { state: 'idle' } | { state: 'sending' } | { state: 'done' } | { state: 'error'; message: string };

export function SaySheet({ health, debug, onClose }: Props) {
    const [text, setText] = useState('');
    const [outcome, setOutcome] = useState<Outcome>({ state: 'idle' });

    const live = debug.sessions?.find((s) => !s.closedAt);
    const speakable = live?.canSpeak === true;

    const send = async () => {
        const value = text.trim();
        if (!value || outcome.state === 'sending') return;
        setOutcome({ state: 'sending' });
        try {
            const { outcome: said } = await api.say(value);
            if (said === 'failed') {
                setOutcome({ state: 'error', message: 'Колонка не ответила — возможно, она сейчас занята. Попробуйте ещё раз.' });
                return;
            }
            setOutcome({ state: 'done' });
            setText('');
        } catch (e) {
            const status = e instanceof ApiError ? e.status : 0;
            setOutcome({
                state: 'error',
                message:
                    status === 404
                        ? 'Нет живой сессии с колонкой.'
                        : status === 403
                          ? problemOf(e)
                          : 'Не получилось — попробуйте ещё раз.',
            });
        }
    };

    return (
        <div
            class="sheet-backdrop"
            onClick={(e) => {
                if (e.target === e.currentTarget) onClose();
            }}
        >
            <div class="sheet glass-strong" role="dialog" aria-label="Сказать через колонку">
                <div class="flex items-center justify-between">
                    <h2 class="t-title-xs m-0">
                        Сказать через колонку
                    </h2>
                    <span class="t-meta">
                        {health?.station?.connected ? 'колонка в сети' : 'колонка не в сети'}
                    </span>
                </div>

                {speakable ? (
                    <>
                        <div class="flex gap-8 items-center">
                            <input
                                value={text}
                                onInput={(e) => {
                                    setText((e.target as HTMLInputElement).value);
                                    setOutcome({ state: 'idle' });
                                }}
                                onKeyDown={(e) => {
                                    if (e.key === 'Enter') void send();
                                }}
                                placeholder="Что сказать?"
                                aria-label="Текст для колонки"
                                autoFocus
                                class="flex-1 min-w-0 py-12 px-16 r-pill border-glass bg-icon-bg outline-none"
                            />
                            <button class="talk-btn w-48 h-48 flex-none" aria-label="Сказать" onClick={() => void send()} disabled={outcome.state === 'sending'}>
                                <IconSend size={20} />
                            </button>
                        </div>
                        {outcome.state === 'done' && <p class="t-meta m-0">Колонка сказала.</p>}
                        {outcome.state === 'error' && <p class="t-meta m-0 c-dot-offline">{outcome.message}</p>}
                    </>
                ) : live ? (
                    <p class="t-meta m-0">
                        Колонка подключена, но шлюз не может говорить за неё (режим релея или нет прав голоса).
                    </p>
                ) : (
                    <p class="t-meta m-0">
                        Нужна живая сессия с колонкой: скажите «Алиса» и попробуйте снова.
                    </p>
                )}
                <Slot name="say.extras" props={{ speakable }} />
            </div>
        </div>
    );
}
