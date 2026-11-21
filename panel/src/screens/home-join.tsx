// «Просятся в дом» (docs/plans/home-setup-flow.md, этап 2; холст «Онбординг», доска PhonePair): карточка центра с
// каждым открытым запросом — кто просится, код крупно, «Разрешить» и «Это не я». Видна администратору на любом экране
// пульта (рамка app.tsx) и в «Устройствах»; на телефоне всё в одну колонку. Код тот же, что сказала колонка или
// показало окно центра: человек сверяет его и разрешает, вводить не нужно.

import { useState } from 'preact/hooks';

import { problemOf } from '../api';
import { answerJoin, codePairs, useHomeJoins } from '../home-join';
import { tr } from '../i18n';
import { clock } from '../time';

export function JoinRequests() {
    const requests = useHomeJoins();
    const [busy, setBusy] = useState<string | null>(null);
    const [error, setError] = useState<string | null>(null);
    if (requests.length === 0) return null;
    const t = tr.devices;
    const answer = async (id: string, allow: boolean): Promise<void> => {
        setBusy(id);
        setError(null);
        try {
            await answerJoin(id, allow);
        } catch (e) {
            setError(problemOf(e));
        } finally {
            setBusy(null);
        }
    };
    return (
        <section class="card glass px-16 py-14 flex flex-col gap-10" aria-label={t.joinTitle} role="status">
            <h2 class="t-title-xs m-0">{t.joinTitle}</h2>
            {requests.map((request) => {
                // По языку пульта, не браузера: «22:58», а не «10:58 PM».
                const until = clock(request.expiresAt);
                return (
                    <div key={request.id} class="flex flex-col gap-8">
                        <span class="w-strong">{request.kind === 'ear' ? t.joinEar(request.name) : t.joinTurbo(request.name)}</span>
                        <span class="w-cap">{t.joinFrom(request.source, until)}</span>
                        {request.code && (
                            // DESIGN.md «figure»: число крупно, цифры одной ширины.
                            <span
                                style={{ font: '300 40px/44px "Inter Tight", Inter, system-ui, sans-serif', letterSpacing: '-0.03em', fontVariantNumeric: 'tabular-nums' }}
                                aria-label={`${t.joinCodeLabel}: ${request.code.split('').join(' ')}`}
                            >
                                {codePairs(request.code)}
                            </span>
                        )}
                        <p class="w-cap m-0">{request.kind === 'ear' ? `${t.joinEarWarn} ${t.joinHint}` : t.joinHint}</p>
                        <div class="flex flex-wrap gap-8">
                            <button type="button" class="btn btn-main" disabled={busy !== null} aria-busy={busy === request.id || undefined} onClick={() => void answer(request.id, true)}>
                                {t.joinAllow}
                            </button>
                            <button type="button" class="btn btn-ghost" disabled={busy !== null} onClick={() => void answer(request.id, false)}>
                                {t.joinDeny}
                            </button>
                        </div>
                    </div>
                );
            })}
            {error && (
                <p class="w-cap m-0" role="alert" style={{ color: 'var(--bad)' }}>
                    {error}
                </p>
            )}
        </section>
    );
}
