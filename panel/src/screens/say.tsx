import { useRef, useState } from 'preact/hooks';

import { ApiError, Health, SessionSnapshot, UnsupportedEntry, api, problemOf } from '../api';
import { IconSend } from '../icons';
import { Slot } from '../shell/slot';
import { tr } from '../i18n';
import { Overlay, useDialogFocus } from '../ui/sheet';

interface Props {
    health: Health | null;
    debug: { sessions: SessionSnapshot[] | null; unsupported: UnsupportedEntry[]; error: unknown };
    onClose: () => void;
}

type Outcome = { state: 'idle' } | { state: 'sending' } | { state: 'done' } | { state: 'error'; message: string };

export function SaySheet({ health, debug, onClose }: Props) {
    const [text, setText] = useState('');
    const [outcome, setOutcome] = useState<Outcome>({ state: 'idle' });
    // Окно как окно: фокус внутрь, Esc закрывает его (а не окно раздела под ним), фокус — обратно.
    const box = useRef<HTMLDivElement>(null);
    useDialogFocus(box, onClose);

    const live = debug.sessions?.find((s) => !s.closedAt);
    const speakable = live?.canSpeak === true;

    const send = async () => {
        const value = text.trim();
        if (!value || outcome.state === 'sending') return;
        setOutcome({ state: 'sending' });
        try {
            const { outcome: said } = await api.say(value);
            if (said === 'failed') {
                setOutcome({ state: 'error', message: tr.say.noAnswer });
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
                        ? tr.say.noSession
                        : status === 403
                          ? problemOf(e)
                          : tr.say.failed,
            });
        }
    };

    return (
        <Overlay onDismiss={onClose}>
            <div ref={box} class="sheet glass-strong" role="dialog" aria-modal="true" aria-label={tr.say.title}>
                <div class="flex items-center justify-between">
                    <h2 class="t-title-xs m-0">
                        {tr.say.title}
                    </h2>
                    <span class="t-meta">
                        {health?.station?.connected ? tr.say.stationOnline : tr.say.stationOffline}
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
                                placeholder={tr.say.placeholder}
                                aria-label={tr.say.inputLabel}
                                autoFocus
                                class="flex-1 min-w-0 py-12 px-16 r-pill border-glass bg-icon-bg outline-none"
                            />
                            <button class="talk-btn w-48 h-48 flex-none" aria-label={tr.say.sendLabel} onClick={() => void send()} disabled={outcome.state === 'sending'}>
                                <IconSend size={20} />
                            </button>
                        </div>
                        {outcome.state === 'done' && <p class="t-meta m-0">{tr.say.said}</p>}
                        {outcome.state === 'error' && <p class="t-meta m-0 c-dot-offline">{outcome.message}</p>}
                    </>
                ) : live ? (
                    <p class="t-meta m-0">
                        {tr.say.cannotSpeak}
                    </p>
                ) : (
                    <p class="t-meta m-0">
                        {tr.say.needSession}
                    </p>
                )}
                <Slot name="say.extras" props={{ speakable }} />
            </div>
        </Overlay>
    );
}
