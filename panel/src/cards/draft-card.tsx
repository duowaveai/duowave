// Карточка черновика (docs/plugins/cards.md): первая из `contributes.panel.cards` его манифеста, данные — из
// песочницы мастерской (на примере, как плитка и «Примерка»). Рисует пульт, кода черновика в пульте нет — показ не
// спрашивает «да». Нажатия зовут функции черновика там же.

import type { ComponentChildren } from 'preact';
import { useEffect, useMemo, useState } from 'preact/hooks';

import { CardSpec, DraftTrying, api, problemOf } from '../api';
import type { SlotContribution } from '../shell/slot';
import { CardData, CardView, cardOf } from './card';
import { tr } from '../i18n';

async function callDraft(id: string, fn: string, input: unknown, trying: DraftTrying): Promise<unknown> {
    const report = await api.runDraft(id, fn, input, trying);
    if (!report.ok) throw new Error(report.error ?? tr.cards.fnFailed(fn));
    return report.result;
}

/** Карточки черновика по его манифесту; нет манифеста или карточек — пусто. */
export async function draftCards(id: string): Promise<CardSpec[]> {
    try {
        const manifest = JSON.parse((await api.readDraftFile(id, 'manifest.json')).content);
        const cards = manifest?.contributes?.panel?.cards;
        return Array.isArray(cards) ? cards.filter((card: CardSpec) => card && typeof card.id === 'string' && typeof card.function === 'string') : [];
    } catch {
        return [];
    }
}

/**
 * Карточка черновика, как она встанет на стол. `nonce` — показать заново после правки; `trying` — на примере и с
 * обычными настройками; `onWidget` — какой вклад стола из неё выйдет (его id ставится на стол); `fallback` — что
 * показать, если карточек у черновика нет (фразы навыка).
 */
export function DraftCardPreview({
    id,
    nonce,
    trying,
    onWidget,
    fallback,
}: {
    id: string;
    nonce: number;
    trying?: DraftTrying;
    onWidget?: (contribution: SlotContribution) => void;
    fallback?: ComponentChildren;
}) {
    const [spec, setSpec] = useState<CardSpec | null | undefined>(undefined);
    const [card, setCard] = useState<CardData | null>(null);
    const [error, setError] = useState<string | null>(null);
    const [busy, setBusy] = useState(false);
    const settings = JSON.stringify(trying?.settings ?? {});
    const how = useMemo<DraftTrying>(() => ({ example: trying?.example ?? true, settings: trying?.settings }), [trying?.example, settings]);
    useEffect(() => {
        let alive = true;
        setError(null);
        setCard(null);
        void (async () => {
            const found = (await draftCards(id))[0] ?? null;
            if (!alive) return;
            setSpec(found);
            if (!found) return;
            onWidget?.({ id: `card.${found.id}`, title: found.title, size: found.size === 'large' ? 'large' : 'small', Component: () => null });
            try {
                const next = cardOf(await callDraft(id, found.function, {}, how));
                if (!alive) return;
                if (next) setCard(next);
                else setError(tr.cards.notACard(found.function));
            } catch (e) {
                if (alive) setError(e instanceof Error && !('status' in e) ? e.message : problemOf(e));
            }
        })();
        return () => {
            alive = false;
        };
    }, [id, nonce, how]);
    const tap = (fn: string, input: unknown = {}): void => {
        setBusy(true);
        setError(null);
        callDraft(id, fn, input, how)
            .then((answer) => {
                const next = cardOf(answer);
                if (next) setCard(next);
            })
            .catch((e) => setError(e instanceof Error ? e.message : problemOf(e)))
            .finally(() => setBusy(false));
    };
    if (spec === undefined) return <p class="t-caption m-0">{tr.cards.loadingCard}</p>;
    if (spec === null) return <>{fallback ?? null}</>;
    return (
        <div class="flex flex-col gap-8">
            {card ? <CardView app={id} card={card} tap={tap} busy={busy} /> : !error && <p class="t-caption m-0">{tr.cards.loading}</p>}
            {error && (
                <p class="t-caption m-0 c-dot-offline" role="alert">
                    {error}
                </p>
            )}
        </div>
    );
}
