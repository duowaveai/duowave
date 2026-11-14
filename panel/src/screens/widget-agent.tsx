// «Новый виджет: голосом или текстом» в «+» (docs/plans/voice-creation.md, «+» остаётся): просьба уходит той же дорогой,
// что у колонки (`POST /api/v1/workshop/make`), панель закрывается — дальше вещь рождается на столе плиткой: комета
// агента, контур, строки по его шагам, потом «на примерке». Своего предпросмотра и «Поставить» у «+» больше нет: их
// место — плитка и «Примерка». Голосом — «Сказать» в доке (микрофон пульта) или колонке: «Алиса, сделай виджет…».

import { useState } from 'preact/hooks';

import { can } from '../api';
import { askToMake, asRequest, failed } from '../voice/creation';
import { useAgentPlace } from '../voice/drafts';
import { useVoiceAction } from '../voice/phrases';
import { tr } from '../i18n';
import { byMode } from '../words';

export { DraftWidgetPreview, mayRunDraft } from './draft-preview';

export function WidgetAgent({ onMade }: { onMade: () => void }) {
    const [request, setRequest] = useState('');
    const [busy, setBusy] = useState(false);
    const [error, setError] = useState<string | null>(null);
    const cloud = useAgentPlace();
    const mayMake = can('plugins.manage');

    const make = async (said?: string) => {
        const text = (said ?? request).trim();
        if (!text || busy) return;
        setBusy(true);
        setError(null);
        try {
            await askToMake(asRequest(text), cloud);
            setRequest('');
            onMade();
        } catch (e) {
            setError(failed(e));
        } finally {
            setBusy(false);
        }
    };
    // «Нарисовать» — фраза кнопки: «нарисуй …» голосом идёт сюда же.
    useVoiceAction(mayMake ? ['нарисуй', 'нарисовать'] : [], (rest) => void make(rest || undefined));

    if (!mayMake) return <p class="w-cap m-0">{tr.drafts.adminOnly}</p>;

    return (
        <section class="widget-agent" aria-labelledby="widget-agent-h">
            <h3 id="widget-agent-h" class="add-sub">
                {tr.drafts.askTitle}
            </h3>
            <form
                class="widget-agent-ask"
                onSubmit={(event) => {
                    event.preventDefault();
                    void make();
                }}
            >
                <textarea
                    class="field"
                    rows={2}
                    value={request}
                    placeholder={tr.drafts.askPlaceholder}
                    aria-label={tr.drafts.askLabel}
                    disabled={busy}
                    onInput={(event) => setRequest((event.target as HTMLTextAreaElement).value)}
                />
                <div class="flex items-center gap-8">
                    <button type="submit" class="w-btn w-btn-ink" disabled={busy || !request.trim()}>
                        {busy ? tr.drafts.handingToAgent : tr.drafts.draw}
                    </button>
                    <span class="w-cap">{byMode(tr.drafts.askedSpeakerHint, tr.drafts.askedVoiceHint)}</span>
                </div>
            </form>
            <p class="w-cap m-0">{tr.drafts.appearsAsTile}</p>
            {error && (
                <p class="w-cap c-dot-offline m-0" role="alert">
                    {error}
                </p>
            )}
        </section>
    );
}
