// Предпросмотр черновика (docs/plans/desk.md, этап 5; docs/plans/voice-creation.md): виджет черновика, как он встанет на
// стол, — его panel.js с шлюза по адресу-токену, данные — из песочницы мастерской. Плитка на столе и «Примерка» зовут
// песочницу на примере (`examples.json`): до «да» черновик в сеть не ходит; мастерская — по сети, как раньше.
//
// panel.js черновика выполняется в самом пульте, с правами вошедшего. Черновик, начатый в пульте, человек попросил сам —
// его показ не переспрашивает (пока код тот же). Черновик колонки (`golos-…`) спрашивает перед каждым показом
// (mayRunDraft): попросить его мог кто угодно рядом с колонкой. Показ выполняет только разрешённый текст panel.js.

import type { FunctionComponent } from 'preact';
import { useEffect, useState } from 'preact/hooks';

import { DraftTrying as Trying, api, problemOf } from '../api';
import { DraftPreview, DraftTrying } from '../sdk';
import type { SlotContribution } from '../shell/slot';
import type { DeskWidget } from '../shell/widgets';
import { ask } from '../ui/confirm';
import { WidgetFrame } from '../ui/widget';
import { gw } from '../via';

/**
 * Чей panel.js человек уже разрешил выполнить в пульте — и какой: разрешение держится за текст кода, и другой код
 * спрашивает снова. До перезагрузки страницы. Черновики, сделанные голосом, сюда не попадают: их каждый показ — вопрос.
 */
const trusted = new Map<string, string>();

/** Черновик сделан голосом у колонки (gateway/src/plugins/workshop/voice.rs: `golos-…`): попросить мог кто угодно. */
export const madeByVoice = (id: string): boolean => id.startsWith('golos-');

/** panel.js черновика, как он лежит сейчас. */
export const panelCode = (id: string): Promise<string> => api.readDraftFile(id, 'panel.js').then((answer) => answer.content);

/** Черновик человек попросил сам, в пульте: его panel.js показывается без вопроса, пока код тот же. */
export async function trustOwnDraft(id: string): Promise<string> {
    const code = await panelCode(id);
    trusted.set(id, code);
    return code;
}

/** Разрешённый уже код черновика, если он тот же, что лежит сейчас; иначе null. */
export async function trustedCode(id: string): Promise<string | null> {
    const before = trusted.get(id);
    if (before === undefined || madeByVoice(id)) return null;
    const code = await panelCode(id);
    return code === before ? code : null;
}

/**
 * Можно ли выполнить panel.js черновика в пульте: код работает с правами того, кто вошёл. Ответ — тот текст panel.js,
 * что человек разрешил (показ выполнит только его), или null. Черновик, сделанный голосом (`golos-…`), спрашивает каждый
 * раз, с прямым предупреждением: попросить его мог кто угодно рядом с колонкой, а его агент правит код и потом. Прочие
 * помнят разрешённый код и спрашивают снова, когда он стал другим.
 */
export async function mayRunDraft(id: string): Promise<string | null> {
    const code = await panelCode(id);
    const voice = madeByVoice(id);
    const before = trusted.get(id);
    if (!voice && before === code) return code;
    trusted.delete(id);
    const yes = await ask({
        title: voice ? 'Показать виджет, сделанный голосом?' : 'Показать виджет черновика?',
        text: [
            voice ? 'Этот черновик сделан голосом у колонки: попросить его мог кто угодно рядом с ней — даже звук из телевизора.' : null,
            !voice && before !== undefined ? 'panel.js поменялся с тех пор, как вы его показывали.' : null,
            'panel.js черновика выполнится в пульте с вашими правами: всё, что можете вы, сможет и он.',
            voice ? 'Сначала прочтите его panel.js в «Файлах». Вопрос будет перед каждым показом.' : null,
        ]
            .filter(Boolean)
            .join(' '),
        ok: 'Показать',
        danger: voice,
    });
    if (!yes) return null;
    if (!voice) trusted.set(id, code);
    return code;
}

/** Стили черновика на странице, пока его предпросмотр открыт. */
function addStyles(href: string): HTMLLinkElement {
    const link = document.createElement('link');
    link.rel = 'stylesheet';
    link.href = gw(href);
    document.head.appendChild(link);
    return link;
}

/**
 * Виджет черновика, как он встанет на стол: его panel.js по адресу-токену, данные — из песочницы. `nonce` — показать
 * заново после правки. `code` — panel.js, который человек разрешил (mayRunDraft): стал другим, пока спрашивали, — код
 * не выполняется. `trying` — на примере и с обычными настройками (плитка, «Примерка»). `bare` — без рамки виджета:
 * рамку даёт плитка. `onWidget` — какой вклад нашёлся (его id ставится на стол). `window` — вместо виджета окно
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
    const [contribution, setContribution] = useState<SlotContribution | null>(null);
    const [screen, setScreen] = useState<{ Screen: FunctionComponent } | null>(null);
    const [error, setError] = useState<string | null>(null);
    useEffect(() => {
        let alive = true;
        let link: HTMLLinkElement | null = null;
        setError(null);
        setContribution(null);
        void (async () => {
            try {
                const { module, styles: css } = await api.draftPreview(id);
                // Код сейчас — тот, что разрешили? Агент мог переписать его, пока человек отвечал.
                const now = await panelCode(id);
                // Закрыли, пока шлюз отвечал: ни стилей, ни кода.
                if (!alive) return;
                if (now !== code) throw new Error('panel.js поменялся, пока вы решали, — нажмите «Показать заново».');
                if (css) link = addStyles(css);
                const loaded = await import(/* @vite-ignore */ gw(module));
                if (!alive) return;
                const plugin = loaded.default ?? loaded.plugin;
                const section: { Screen: FunctionComponent } | undefined = plugin?.sections?.[0];
                setScreen(section ? { Screen: section.Screen } : null);
                onWindow?.(Boolean(section));
                const found: SlotContribution | undefined = plugin?.slots?.['desk.widgets']?.[0];
                if (!found) throw new Error('В черновике нет виджета стола (слот desk.widgets).');
                setContribution(found);
                onWidget?.(found);
            } catch (e) {
                if (alive) setError(e instanceof Error && !('status' in e) ? e.message : problemOf(e));
            }
        })();
        return () => {
            alive = false;
            link?.remove();
        };
    }, [id, nonce, code]);
    if (error) return <p class="t-caption c-dot-offline m-0">{error}</p>;
    if (!contribution) return <p class="t-caption m-0">Загружаю виджет…</p>;
    if (asWindow && screen) {
        const { Screen } = screen;
        return (
            <DraftPreview.Provider value={id}>
                <DraftTrying.Provider value={trying ?? {}}>
                    <div class="fitting-window">
                        <Screen />
                    </div>
                </DraftTrying.Provider>
            </DraftPreview.Provider>
        );
    }
    const { Component } = contribution;
    const widget: DeskWidget = { ...contribution, plugin: `package.${id}`, key: `preview.${id}`, size: contribution.size ?? 'small', initial: true };
    const body = (
        <DraftPreview.Provider value={id}>
            <DraftTrying.Provider value={trying ?? {}}>
                {bare ? (
                    <Component />
                ) : (
                    <WidgetFrame widget={widget} menu={{ hide: () => setContribution(null) }}>
                        <Component />
                    </WidgetFrame>
                )}
            </DraftTrying.Provider>
        </DraftPreview.Provider>
    );
    return bare ? body : <div class="widget-agent-preview">{body}</div>;
}
