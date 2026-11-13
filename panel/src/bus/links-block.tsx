// «Связи» в окне приложения (docs/plans/bus.md, «Пульт»): с кем оно общается и на какой ступени — меняется выбором;
// «Общается с другими» — выключатель («Отдельно»); его режим или режим дома; что оно попросило в манифесте; вопросы,
// что ждут вашего «да»; что оно видит сейчас; последние письма в журнале. Всё решает шлюз: блок только показывает и
// передаёт ваше слово.

import { useEffect, useState } from 'preact/hooks';

import { BusAppView, BusJournalLine, BusLevel, BusLink, BusMode, api, can, problemOf } from '../api';
import { timeAgo } from '../time';
import { Action } from '../ui/cards';
import { Switch } from '../ui/switch';
import { busChanged, useBusVersion } from './state';
import { KIND_TITLES, LEVELS, MODE_HINTS, MODE_TITLES, OUTCOME_TITLES, SOURCE_TITLES, levelWord } from './words';

/** Право шины из манифеста словами: `call:app/notes` → «звать app/notes». */
function askedWords(right: string, titles: Map<string, string>): string {
    const [level, pattern] = right.split(/:(.*)/s, 2) as [BusLevel, string];
    const name = titles.get(pattern) ?? (pattern === '*' ? 'всё' : pattern.endsWith('/*') ? `всё в ${pattern.slice(0, -2)}` : pattern);
    const verb: Record<string, string> = { see: 'видеть', listen: 'слышать', call: pattern.startsWith('files/') ? 'читать' : 'звать', edit: pattern.startsWith('files/') ? 'менять файлы в' : 'править' };
    return `${verb[level] ?? level} «${name}»`;
}

function LinkRow({ link, me, titles, onChanged }: { link: BusLink; me: string; titles: Map<string, string>; onChanged: () => void }) {
    const [busy, setBusy] = useState(false);
    const [error, setError] = useState<string | null>(null);
    const outgoing = link.from === me;
    const other = outgoing ? link.to : link.from;
    const name = titles.get(other) ?? other;
    const run = async (action: () => Promise<unknown>): Promise<void> => {
        setBusy(true);
        setError(null);
        try {
            await action();
            busChanged();
            onChanged();
        } catch (e) {
            setError(problemOf(e));
        } finally {
            setBusy(false);
        }
    };
    return (
        <div class="flex flex-col gap-4">
            <div class="flex flex-wrap items-center gap-8">
                <span class="t-body-sm flex-1 min-w-0">{outgoing ? `→ «${name}»` : `«${name}» →`}</span>
                <select
                    class="field"
                    aria-label={`Ступень связи с «${name}»`}
                    value={link.level}
                    disabled={busy}
                    onChange={(e) => void run(() => api.busLink(link.from, link.to, (e.target as HTMLSelectElement).value as BusLevel))}
                >
                    {LEVELS.map((level) => (
                        <option key={level} value={level}>
                            {levelWord(level, link.to)}
                        </option>
                    ))}
                </select>
                <Action label="Убрать" disabled={busy} onClick={() => void run(() => api.busUnlink(link.from, link.to))} />
            </div>
            <span class="t-caption">{SOURCE_TITLES[link.source] ?? link.source}{link.at ? ` · ${timeAgo(link.at)}` : ''}</span>
            {error && (
                <span class="t-caption c-dot-offline" role="alert">
                    {error}
                </span>
            )}
        </div>
    );
}

function JournalLines({ me, titles }: { me: string; titles: Map<string, string> }) {
    const version = useBusVersion();
    const [lines, setLines] = useState<BusJournalLine[] | null>(null);
    useEffect(() => {
        api.busJournal(me, undefined, 20)
            .then((answer) => setLines(answer.lines))
            .catch(() => setLines([]));
    }, [me, version]);
    if (!lines) return <span class="t-caption">Читаю журнал…</span>;
    if (lines.length === 0) return <span class="t-caption">Писем пока не было.</span>;
    return (
        <ul class="flex flex-col gap-4 m-0 p-0" style={{ listStyle: 'none' }}>
            {lines.map((line) => (
                <li key={line.seq} class="t-caption">
                    {timeAgo(line.at)} · {line.from === me ? `→ «${titles.get(line.to) ?? line.to}»` : `«${titles.get(line.from) ?? line.from}» →`} ·{' '}
                    {KIND_TITLES[line.kind] ?? line.kind} {line.handle} · {OUTCOME_TITLES[line.outcome] ?? line.outcome}
                </li>
            ))}
        </ul>
    );
}

/** «Связи» приложения `id` — администратору дома. */
export function LinksBlock({ id }: { id: string }) {
    const version = useBusVersion();
    const me = `app/${id}`;
    const [view, setView] = useState<BusAppView | null>(null);
    const [titles, setTitles] = useState<Map<string, string>>(new Map());
    const [error, setError] = useState<string | null>(null);
    const [busy, setBusy] = useState(false);
    const [journal, setJournal] = useState(false);
    const [nonce, setNonce] = useState(0);
    useEffect(() => {
        let alive = true;
        Promise.all([api.busApp(id), api.bus()])
            .then(([app, all]) => {
                if (!alive) return;
                setView(app);
                setTitles(new Map(all.participants.map((participant) => [participant.address, participant.title])));
                setError(null);
            })
            .catch((e) => alive && setError(problemOf(e)));
        return () => {
            alive = false;
        };
    }, [id, version, nonce]);
    if (!can('plugins.manage')) return null;
    if (error && !view) return <span class="t-caption">Связи: {error}</span>;
    if (!view) return null;
    const mode: BusMode = view.mode;
    const run = async (action: () => Promise<unknown>): Promise<void> => {
        setBusy(true);
        setError(null);
        try {
            await action();
            busChanged();
            setNonce((n) => n + 1);
        } catch (e) {
            setError(problemOf(e));
        } finally {
            setBusy(false);
        }
    };
    const answer = (question: string, yes: boolean) => run(() => api.busAnswer(question, yes));
    const reaches = view.reaches.filter((visible) => visible.address !== me);
    return (
        <div class="flex flex-col gap-8">
            <h3 class="t-title-xs m-0">Связи</h3>
            <div class="flex items-center gap-12">
                <span class="t-body-sm flex-1">Общается с другими</span>
                <Switch
                    on={mode !== 'isolated'}
                    label="Общается с другими"
                    disabled={busy}
                    onChange={(on) => void run(() => api.busMode(me, on ? (view.ownMode ? null : 'rules') : 'isolated'))}
                />
            </div>
            {mode !== 'isolated' && (
                <div class="flex flex-wrap items-center gap-8">
                    {(['rules', 'free'] as BusMode[]).map((option) => (
                        <Action key={option} on={mode === option} label={MODE_TITLES[option]} disabled={busy} onClick={() => void run(() => api.busMode(me, option))} />
                    ))}
                    {view.ownMode && <Action label="Как у дома" disabled={busy} onClick={() => void run(() => api.busMode(me, null))} />}
                </div>
            )}
            <span class="t-caption">
                {MODE_HINTS[mode]}
                {view.ownMode ? '' : ' Это режим дома.'}
            </span>
            {view.questions.map((question) => (
                <div key={question.id} class="flex flex-col gap-4">
                    <span class="t-body-sm">Ждёт вашего «да»: {question.what}.</span>
                    <div class="flex flex-wrap gap-8">
                        <Action tone="on" label={question.kind === 'once' ? 'Разрешить один раз' : 'Разрешить'} disabled={busy} onClick={() => void answer(question.id, true)} />
                        <Action label="Не разрешать" disabled={busy} onClick={() => void answer(question.id, false)} />
                    </div>
                </div>
            ))}
            {view.asked.length > 0 && <span class="t-caption">Просит в манифесте: {view.asked.map((right) => askedWords(right, titles)).join(', ')}.</span>}
            {view.links.length > 0 ? (
                view.links.map((link) => <LinkRow key={`${link.from}>${link.to}`} link={link} me={me} titles={titles} onChanged={() => setNonce((n) => n + 1)} />)
            ) : (
                <span class="t-caption">Своих связей нет: только то, что дают его права из установки.</span>
            )}
            {reaches.length > 0 && (
                <span class="t-caption">
                    Видит сейчас: {reaches.map((visible) => `«${visible.title}»${visible.handles.some((handle) => handle.allowed && handle.level !== 'see') ? '' : ' (только видит)'}`).join(', ')}.
                </span>
            )}
            <div class="flex flex-wrap gap-8">
                <Action label={journal ? 'Скрыть журнал' : 'Журнал связей'} onClick={() => setJournal((open) => !open)} />
            </div>
            {journal && <JournalLines me={me} titles={titles} />}
            {error && (
                <span class="t-caption c-dot-offline" role="alert">
                    {error}
                </span>
            )}
        </div>
    );
}
