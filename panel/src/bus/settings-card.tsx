// «Связи» в настройках (docs/plans/bus.md, решения 1 и 4): режим дома для всех приложений, «Вопросы словами через
// облако» — с прямым предупреждением, что приложения, спрашивая друг друга словами, могут отдать облачной модели то,
// что знают, — и журнал шины: кто кому писал и чем кончилось, без содержимого.

import { useEffect, useState } from 'preact/hooks';

import { BusJournalLine, BusMode, BusOverview, api, can, problemOf } from '../api';
import { IconJournal } from '../icons';
import { tr } from '../i18n';
import { timeAgo } from '../time';
import { Action } from '../ui/cards';
import { Sheet } from '../ui/sheet';
import { Switch } from '../ui/switch';
import { busChanged, useBusVersion } from './state';
import { KIND_TITLES, MODE_HINTS, MODE_TITLES, OUTCOME_TITLES, rootWord } from './words';

function JournalSheet({ titles, onClose }: { titles: Map<string, string>; onClose: () => void }) {
    const [lines, setLines] = useState<BusJournalLine[] | null>(null);
    const [error, setError] = useState<string | null>(null);
    const [more, setMore] = useState(true);
    const read = (before?: number): void => {
        api.busJournal(undefined, before, 100)
            .then((answer) => {
                setLines((had) => (before != null ? [...(had ?? []), ...answer.lines] : answer.lines));
                setMore(answer.lines.length === 100);
            })
            .catch((e) => setError(problemOf(e)));
    };
    useEffect(() => read(), []);
    const name = (address: string): string => titles.get(address) ?? address;
    return (
        <Sheet title={tr.bus.journalTitle} about={tr.bus.journalAbout} icon={<span class="route-icon w-38 h-38 flex-none" aria-hidden="true"><IconJournal size={19} /></span>} onClose={onClose}>
            {error && (
                <p class="t-caption m-0 c-dot-offline" role="alert">
                    {error}
                </p>
            )}
            {!lines && !error && <p class="t-caption m-0">{tr.bus.reading}</p>}
            {lines && lines.length === 0 && <p class="t-caption m-0">{tr.bus.noLetters}</p>}
            {lines && lines.length > 0 && (
                <ul class="flex flex-col gap-8 m-0 p-0" style={{ listStyle: 'none' }}>
                    {lines.map((line) => (
                        <li key={line.seq} class="flex flex-col gap-2">
                            <span class="t-body-sm">
                                {tr.bus.journalLine(name(line.from), KIND_TITLES[line.kind] ?? line.kind, name(line.to), line.handle ?? '')}
                            </span>
                            <span class="t-caption">
                                {tr.bus.journalMeta(timeAgo(line.at), OUTCOME_TITLES[line.outcome] ?? line.outcome, rootWord(line.root), line.ms)}
                                {line.note ? ` · ${line.note}` : ''}
                            </span>
                        </li>
                    ))}
                </ul>
            )}
            {lines && more && lines.length > 0 && <Action label={tr.bus.more} onClick={() => read(lines[lines.length - 1].seq)} />}
        </Sheet>
    );
}

/** Карточка «Связи» в настройках: администратору дома. */
export function BusCard() {
    const version = useBusVersion();
    const [view, setView] = useState<BusOverview | null>(null);
    const [busy, setBusy] = useState(false);
    const [error, setError] = useState<string | null>(null);
    const [journal, setJournal] = useState(false);
    useEffect(() => {
        api.bus()
            .then(setView)
            .catch((e) => setError(problemOf(e)));
    }, [version]);
    if (!can('plugins.manage')) return null;
    const run = async (action: () => Promise<unknown>): Promise<void> => {
        setBusy(true);
        setError(null);
        try {
            await action();
            busChanged();
            setView(await api.bus());
        } catch (e) {
            setError(problemOf(e));
        } finally {
            setBusy(false);
        }
    };
    const house: BusMode = view?.modes.house ?? 'rules';
    const titles = new Map((view?.participants ?? []).map((participant) => [participant.address, participant.title]));
    const cloud = view?.options.cloudAsks ?? false;
    return (
        <>
            <section class="card glass card-list" aria-labelledby="bus-h">
                <div class="section-head">
                    <h2 id="bus-h" class="t-title-xs m-0">
                        {tr.bus.appLinks}
                    </h2>
                    {can('journal.view') && (
                        <button class="pill pill-off pill-btn" onClick={() => setJournal(true)}>
                            {tr.bus.journalPill}
                        </button>
                    )}
                </div>
                <p class="t-caption m-0">{tr.bus.appLinksNote}</p>
                <div class="flex flex-wrap items-center gap-8">
                    {(['rules', 'free', 'isolated'] as BusMode[]).map((mode) => (
                        <Action key={mode} on={house === mode} label={MODE_TITLES[mode]} disabled={busy || !view} onClick={() => void run(() => api.busMode(null, mode))} />
                    ))}
                </div>
                <p class="t-caption m-0">{MODE_HINTS[house]}</p>
                <div class="flex items-center gap-12">
                    <span class="t-body-sm flex-1">{tr.bus.cloudAsks}</span>
                    <Switch on={cloud} label={tr.bus.cloudAsks} disabled={busy || !view} onChange={(on) => void run(() => api.busOptions({ cloudAsks: on }))} />
                </div>
                <p class={`t-caption m-0 ${cloud ? 'c-pill-yandex-fg' : ''}`}>
                    {tr.bus.cloudAsksNote}
                </p>
                {view && view.questions.length > 0 && <p class="t-caption m-0">{tr.bus.waitingYes(view.questions.length)}</p>}
                {error && (
                    <p class="t-caption m-0 c-dot-offline" role="alert">
                        {error}
                    </p>
                )}
            </section>
            {journal && <JournalSheet titles={titles} onClose={() => setJournal(false)} />}
        </>
    );
}
