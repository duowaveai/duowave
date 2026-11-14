// Предпросмотр черновика (docs/plans/desk.md, этап 5; docs/plans/voice-creation.md): виджет черновика, как он встанет на
// стол, — его panel.js с шлюза по адресу-токену, данные — из песочницы мастерской. Плитка на столе и «Примерка» зовут
// песочницу на примере (`examples.json`): до «да» черновик в сеть не ходит; мастерская — по сети, как раньше.
//
// panel.js черновика работает в своём окошке — изолированном фрейме (docs/plans/bus.md, «Окошки»; frame/host.tsx):
// ни ключа входа, ни хранилища пульта у него нет, в сеть он не ходит, зовёт только функции своего черновика в
// песочнице. Поэтому показ больше не переспрашивает — даже черновик, сделанный голосом у колонки.

import { useEffect, useState } from 'preact/hooks';

import { DraftTrying as Trying, api, problemOf } from '../api';
import { AppFrame, probeShape, probeState } from '../frame/host';
import type { FrameShape, FrameSlotEntry } from '../frame/protocol';
import type { SlotContribution } from '../shell/slot';
import type { DeskWidget } from '../shell/widgets';
import { WidgetFrame } from '../ui/widget';
import { tr } from '../i18n';
import { gw } from '../via';

/** Черновик сделан голосом у колонки (gateway/src/plugins/workshop/voice.rs: `golos-…`): попросить мог кто угодно. */
export const madeByVoice = (id: string): boolean => id.startsWith('golos-');

/** panel.js черновика, как он лежит сейчас. */
export const panelCode = (id: string): Promise<string> => api.readDraftFile(id, 'panel.js').then((answer) => answer.content);

/** Код черновика, который покажется: окошко изолировано, спрашивать не о чем. */
export async function trustOwnDraft(id: string): Promise<string> {
    return panelCode(id);
}

/** Код черновика сейчас (прежде — «разрешённый уже»: окошко изолировано, разрешать нечего). */
export async function trustedCode(id: string): Promise<string | null> {
    return panelCode(id);
}

/** Можно ли показать panel.js черновика: всегда — он работает в окошке, а не в пульте. Ответ — тот текст, что покажется. */
export async function mayRunDraft(id: string): Promise<string | null> {
    return panelCode(id);
}

/** Функция черновика — в песочнице мастерской. */
async function callDraft(id: string, fn: string, input: unknown, trying: Trying): Promise<unknown> {
    const report = await api.runDraft(id, fn, input, trying);
    if (!report.ok) throw new Error(report.error ?? tr.drafts.callFailed(fn));
    return report.result;
}

/** Вклад стола из того, что окошко рассказало о себе: id, размер, подпись — для «На стол». */
function contributionOf(entry: FrameSlotEntry, Component: SlotContribution['Component']): SlotContribution {
    return {
        id: entry.id,
        title: entry.title,
        size: entry.size,
        source: entry.source,
        initial: entry.initial,
        roles: entry.roles,
        bare: entry.bare,
        order: entry.order,
        Component,
    };
}

/**
 * Виджет черновика, как он встанет на стол: его panel.js по адресу-токену в своём окошке, данные — из песочницы.
 * `nonce` — показать заново после правки. `code` — panel.js, который покажется: стал другим, пока шлюз отвечал, — показ
 * просит «Показать заново». `trying` — на примере и с обычными настройками (плитка, «Примерка»). `bare` — без рамки
 * виджета: рамку даёт плитка. `onWidget` — какой вклад нашёлся (его id ставится на стол). `window` — вместо виджета окно
 * приложения (его первый раздел) на том же примере: «Развернуть» в «Примерке» (docs/plans/midi.md); `onWindow` — есть ли
 * у черновика окно.
 */
export function DraftWidgetPreview({
    id,
    nonce,
    code,
    trying,
    bare,
    onWidget,
    window: asWindow,
    onWindow,
}: {
    id: string;
    nonce: number;
    code: string;
    trying?: Trying;
    bare?: boolean;
    onWidget?: (contribution: SlotContribution) => void;
    window?: boolean;
    onWindow?: (has: boolean) => void;
}) {
    const [found, setFound] = useState<{ module: string; styles: string | null; shape: FrameShape; widget: FrameSlotEntry } | null>(null);
    const [error, setError] = useState<string | null>(null);
    const [hidden, setHidden] = useState(false);
    const settings = JSON.stringify(trying?.settings ?? {});
    useEffect(() => {
        let alive = true;
        setError(null);
        setFound(null);
        setHidden(false);
        void (async () => {
            try {
                const { module, styles } = await api.draftPreview(id);
                // Код сейчас — тот, что собирались показать? Агент мог переписать его, пока шлюз отвечал.
                const now = await panelCode(id);
                if (!alive) return;
                if (now !== code) throw new Error(tr.drafts.codeChanged);
                const shape = await probeShape(id, gw(module), styles ? gw(styles) : null, { ...probeState(id, trying?.settings ?? {}), draft: true });
                if (!alive) return;
                onWindow?.(shape.sections.length > 0);
                const widget = shape.slots['desk.widgets']?.[0];
                if (!widget) throw new Error(tr.drafts.noDeskWidget);
                const next = { module: gw(module), styles: styles ? gw(styles) : null, shape, widget };
                setFound(next);
                onWidget?.(contributionOf(widget, () => <DraftFrame id={id} found={next} part={{ kind: 'slot', name: 'desk.widgets', id: widget.id }} trying={trying} />));
            } catch (e) {
                if (alive) setError(e instanceof Error && !('status' in e) ? e.message : problemOf(e));
            }
        })();
        return () => {
            alive = false;
        };
    }, [id, nonce, code, settings, trying?.example]);
    if (error) return <p class="t-caption c-dot-offline m-0">{error}</p>;
    if (!found || hidden) return found ? null : <p class="t-caption m-0">{tr.drafts.loadingWidget}</p>;
    const section = found.shape.sections[0];
    if (asWindow && section) {
        return (
            <div class="fitting-window">
                <DraftFrame id={id} found={found} part={{ kind: 'section', id: section.id }} trying={trying} minHeight={320} />
            </div>
        );
    }
    const body = <DraftFrame id={id} found={found} part={{ kind: 'slot', name: 'desk.widgets', id: found.widget.id }} trying={trying} />;
    if (bare) return body;
    const widget: DeskWidget = {
        ...contributionOf(found.widget, () => body),
        plugin: `package.${id}`,
        key: `preview.${id}`,
        size: found.widget.size ?? 'small',
        initial: true,
    };
    return (
        <div class="widget-agent-preview">
            <WidgetFrame widget={widget} menu={{ hide: () => setHidden(true) }}>
                {body}
            </WidgetFrame>
        </div>
    );
}

/** Окошко черновика: его код в изолированном фрейме, функции — в песочнице мастерской. */
function DraftFrame({
    id,
    found,
    part,
    trying,
    minHeight,
}: {
    id: string;
    found: { module: string; styles: string | null };
    part: { kind: 'section'; id: string } | { kind: 'slot'; name: string; id: string };
    trying?: Trying;
    minHeight?: number;
}) {
    return <AppFrame id={id} module={found.module} styles={found.styles} part={part} draft call={(fn, input) => callDraft(id, fn, input, trying ?? {})} minHeight={minHeight} />;
}
