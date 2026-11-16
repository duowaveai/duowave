// «Поставить на стол?» (docs/plans/start-desk.md, этап 5; доска «Тот же стол через неделю» холста «Стартовый стол —
// лаконично»): приложения сами на стол не встают, но когда человек второй раз спросил то, на что отвечает приложение с
// карточкой или виджетом стола, под сценой появляется строка «Вы дважды спрашивали про «Курсы валют». Поставить на
// стол?» — «Поставить» ставит его карточку с меткой «новый», «Не надо» — больше про это приложение не спрашивает.
// Какое приложение ответило, журнал говорит сам: шаг запроса, который сделало приложение (его фраза или его инструмент,
// позванный моделью), несёт `app`. Шлюз постарше этого не пишет — тогда пульт сверяет фразы приложений
// (`contributes.phrases` из списка пакетов) с вопросом. Что отклонили — в памяти пульта у человека (ключ `deskAsk`).
// Убранную руками карточку строка не предлагает: человек уже решил.

import { useMemo, useState } from 'preact/hooks';

import { PackageView, RequestView, api, can } from '../api';
import { tr } from '../i18n';
import { usePoll } from '../poll';
import { usePanel } from '../shell/context';
import { recalled, remember, useRecalled } from '../shell/memory';
import { DeskWidget, onDesk, useDeskLayout, useWidgets } from '../shell/widgets';
import '../ui/desk-ask.css';

/** Вопросы старше этого не в счёт: «второй раз» — за последние две недели. */
const WINDOW_MS = 14 * 24 * 60 * 60 * 1000;
/** Со второго такого вопроса. */
const ENOUGH = 2;
const KEY = 'deskAsk';

interface Memo {
    /** Приложения, на которые сказали «Не надо». */
    dismissed?: string[];
}

/** Фраза к сравнению: строчные, «ё» как «е», без знаков, пробелы по одному. Скобки слотов `{…}` остаются. */
const norm = (text: string): string =>
    text
        .toLowerCase()
        .replace(/ё/g, 'е')
        .replace(/[^\p{L}\p{N}{} ]+/gu, ' ')
        .replace(/\s+/g, ' ')
        .trim();

const escape = (text: string): string => text.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');

/** Фраза приложения как образец: слот `{…}` — любые слова. Совпадение — фраза целиком. */
function patternOf(phrase: string): RegExp | null {
    const parts = norm(phrase).split(/\{[^}]*\}/);
    if (!parts.join('').trim()) return null;
    return new RegExp(`^${parts.map(escape).join('.+')}$`, 'u');
}

/** Приложение пакета у вклада стола: `package.<id>` — `<id>`; встроенное в пульт — null. */
const appOf = (widget: DeskWidget): string | null => (widget.plugin.startsWith('package.') ? widget.plugin.slice('package.'.length) : null);

export interface DeskOffer {
    app: string;
    title: string;
    widget: DeskWidget;
    times: number;
}

/**
 * Что предложить поставить на стол: приложение с карточкой или виджетом стола, которых на столе нет и которые не
 * убирали, на чьи фразы за две недели спросили хотя бы дважды; из таких — то, о чём спросили последним.
 */
export function offerOf(requests: RequestView[], packages: PackageView[], widgets: DeskWidget[], placed: (widget: DeskWidget) => boolean, hidden: string[], dismissed: string[], now = Date.now()): DeskOffer | null {
    const candidates = new Map<string, DeskWidget>();
    for (const widget of widgets) {
        const app = appOf(widget);
        if (!app || candidates.has(app) || dismissed.includes(app)) continue;
        candidates.set(app, widget);
    }
    // Приложение, у которого хоть один вклад уже на столе или убран руками, не предлагается.
    for (const widget of widgets) {
        const app = appOf(widget);
        if (app && (placed(widget) || hidden.includes(widget.key))) candidates.delete(app);
    }
    if (candidates.size === 0) return null;
    const patterns = new Map<string, RegExp[]>();
    for (const entry of packages) {
        if (!entry.enabled || !candidates.has(entry.id)) continue;
        const list = (entry.contributes?.phrases ?? []).map(patternOf).filter((pattern): pattern is RegExp => pattern !== null);
        if (list.length) patterns.set(entry.id, list);
    }
    const counted = new Map<string, { times: number; last: number }>();
    for (const request of requests) {
        if (request.kind === 'say' || !request.text) continue;
        const at = Date.parse(request.startedAt);
        if (!Number.isFinite(at) || now - at > WINDOW_MS) continue;
        const count = (app: string) => {
            const seen = counted.get(app) ?? { times: 0, last: 0 };
            counted.set(app, { times: seen.times + 1, last: Math.max(seen.last, at) });
        };
        // Журнал назвал приложение — оно и ответило; не назвал (шлюз постарше) — по фразам.
        const named = request.steps?.find((step) => step.app)?.app;
        if (named) {
            if (candidates.has(named)) count(named);
            continue;
        }
        const text = norm(request.text);
        const matched = [...patterns].find(([, list]) => list.some((pattern) => pattern.test(text)));
        if (matched) count(matched[0]);
    }
    let best: DeskOffer | null = null;
    let bestAt = 0;
    for (const [app, { times, last }] of counted) {
        if (times < ENOUGH || last < bestAt) continue;
        const widget = candidates.get(app);
        const title = packages.find((entry) => entry.id === app)?.title ?? widget?.title ?? app;
        if (!widget) continue;
        best = { app, title, widget, times };
        bestAt = last;
    }
    return best;
}

/** Строка под сценой: «Вы дважды спрашивали про «…». Поставить на стол?» */
export function DeskAsk() {
    const t = tr.deskAsk;
    const panel = usePanel();
    const role = panel.role.role;
    const widgets = useWidgets(role, panel.health);
    const [layout, actions] = useDeskLayout(role);
    const memo = useRecalled<Memo>(KEY) ?? {};
    const [requests, setRequests] = useState<RequestView[]>([]);
    const [packages, setPackages] = useState<PackageView[]>([]);
    const allowed = role === 'gateway' && can('journal.view') && can('connectors.view');
    usePoll({
        on: allowed,
        every: 60_000,
        version: panel.version,
        read: (signal) =>
            Promise.all([api.requests({ limit: 80 }), api.packages()]).then(([journal, list]) => {
                if (signal.aborted) return;
                setRequests(journal.requests);
                setPackages(list.packages);
            }),
    });
    const offer = useMemo(
        () => (allowed ? offerOf(requests, packages, widgets, (widget) => onDesk(widget, layout), layout.hidden, memo.dismissed ?? []) : null),
        [allowed, requests, packages, widgets, layout, memo.dismissed],
    );
    if (!offer) return null;
    const Icon = offer.widget.icon;
    const dismiss = () => remember(KEY, { ...(recalled<Memo>(KEY) ?? {}), dismissed: [...new Set([...(recalled<Memo>(KEY)?.dismissed ?? []), offer.app])] });
    return (
        <div class="desk-ask" role="status">
            {Icon && <Icon size={15} />}
            <span class="desk-ask-text">{t.asked(offer.title, offer.times)}</span>
            <button type="button" class="desk-ask-yes" onClick={() => actions.land(offer.widget.key)}>
                {t.put}
            </button>
            <button type="button" class="desk-ask-no" onClick={dismiss}>
                {t.no}
            </button>
        </div>
    );
}
