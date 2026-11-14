// «Связи» в окне приложения (docs/plans/bus.md, «Пульт»): с кем оно общается и на какой ступени — меняется выбором;
// «Общается с другими» — выключатель («Отдельно»); его режим или режим дома; что оно попросило в манифесте; вопросы,
// что ждут вашего «да»; что оно видит сейчас; последние письма в журнале. Всё решает шлюз: блок только показывает и
// передаёт ваше слово.

import { useEffect, useState } from 'preact/hooks';

import { BusAppView, BusJournalLine, BusLevel, BusLink, BusMode, api, can, problemOf } from '../api';
import { tr } from '../i18n';
import { timeAgo } from '../time';
import { Action } from '../ui/cards';
import { Switch } from '../ui/switch';
import { busChanged, useBusVersion } from './state';
import { KIND_TITLES, LEVELS, MODE_HINTS, MODE_TITLES, OUTCOME_TITLES, SOURCE_TITLES, levelWord } from './words';

/** Образец адреса словами: `files/home/*` — «все папки дома», `files/home/Фото/*` — «папка «Фото» со всем в ней». */
export function patternTitle(pattern: string, titles: Map<string, string>): string {
    const known = titles.get(pattern);
    if (known) return known;
    if (pattern === '*') return tr.bus.patternAll;
    const base = pattern.endsWith('/*') ? pattern.slice(0, -2) : pattern;
    const named = titles.get(base);
    if (base === 'files/home') return tr.bus.patternHomeFolders;
    if (base === 'files/home/Подключено') return tr.bus.patternConnected;
    if (base === 'files/project') return tr.bus.patternProjects;
    if (base === 'app') return tr.bus.patternApps;
    if (base.startsWith('files/home/')) return tr.bus.patternFolder(named ?? base.split('/').pop() ?? base, pattern.endsWith('/*'));
    if (named) return pattern.endsWith('/*') ? tr.bus.patternNamedAll(named) : named;
    return pattern.endsWith('/*') ? tr.bus.patternAllIn(base) : pattern;
}

/** Право шины из манифеста словами: `call:app/notes` → «звать app/notes». */
function askedWords(right: string, titles: Map<string, string>): string {
    const [level, pattern] = right.split(/:(.*)/s, 2) as [BusLevel, string];
    const name = patternTitle(pattern, titles);
    const files = pattern.startsWith('files/');
    const verb: Record<string, string> = {
        see: tr.bus.askSee,
        listen: tr.bus.askListen,
        call: files ? tr.bus.askCallFiles : tr.bus.askCall,
        edit: files ? tr.bus.askEditFiles : tr.bus.askEdit,
    };
    return tr.bus.askedRight(verb[level] ?? level, name);
}

function LinkRow({ link, me, titles, onChanged }: { link: BusLink; me: string; titles: Map<string, string>; onChanged: () => void }) {
    const [busy, setBusy] = useState(false);
    const [error, setError] = useState<string | null>(null);
    const outgoing = link.from === me;
    const other = outgoing ? link.to : link.from;
    const name = patternTitle(other, titles);
    // A built-in app's right from its install is not a written link: closing it is a link of yours, «не знает».
    const fromInstall = link.source === 'install';
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
                <span class="t-body-sm flex-1 min-w-0">{outgoing ? tr.bus.outgoing(name) : tr.bus.incoming(name)}</span>
                <select
                    class="field"
                    aria-label={tr.bus.levelLabel(name)}
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
                <Action
                    label={fromInstall ? tr.bus.close : tr.bus.remove}
                    disabled={busy || (fromInstall && link.level === 'none')}
                    onClick={() => void run(() => (fromInstall ? api.busLink(link.from, link.to, 'none') : api.busUnlink(link.from, link.to)))}
                />
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
    if (!lines) return <span class="t-caption">{tr.bus.readingJournal}</span>;
    if (lines.length === 0) return <span class="t-caption">{tr.bus.noLetters}</span>;
    return (
        <ul class="flex flex-col gap-4 m-0 p-0" style={{ listStyle: 'none' }}>
            {lines.map((line) => (
                <li key={line.seq} class="t-caption">
                    {timeAgo(line.at)} · {line.from === me ? tr.bus.outgoing(titles.get(line.to) ?? line.to) : tr.bus.incoming(titles.get(line.from) ?? line.from)} ·{' '}
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
    if (error && !view) return <span class="t-caption">{tr.bus.linksError(error)}</span>;
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
            <h3 class="t-title-xs m-0">{tr.bus.links}</h3>
            <div class="flex items-center gap-12">
                <span class="t-body-sm flex-1">{tr.bus.talksToOthers}</span>
                <Switch
                    on={mode !== 'isolated'}
                    label={tr.bus.talksToOthers}
                    disabled={busy}
                    onChange={(on) => void run(() => api.busMode(me, on ? (view.ownMode ? null : 'rules') : 'isolated'))}
                />
            </div>
            {mode !== 'isolated' && (
                <div class="flex flex-wrap items-center gap-8">
                    {(['rules', 'free'] as BusMode[]).map((option) => (
                        <Action key={option} on={mode === option} label={MODE_TITLES[option]} disabled={busy} onClick={() => void run(() => api.busMode(me, option))} />
                    ))}
                    {view.ownMode && <Action label={tr.bus.likeHome} disabled={busy} onClick={() => void run(() => api.busMode(me, null))} />}
                </div>
            )}
            <span class="t-caption">
                {MODE_HINTS[mode]}
                {view.ownMode ? '' : tr.bus.homeMode}
            </span>
            {view.questions.map((question) => (
                <div key={question.id} class="flex flex-col gap-4">
                    <span class="t-body-sm">{tr.bus.waitsYes(question.what)}</span>
                    <div class="flex flex-wrap gap-8">
                        <Action tone="on" label={question.kind === 'once' ? tr.bus.allowOnce : tr.bus.allow} disabled={busy} onClick={() => void answer(question.id, true)} />
                        <Action label={tr.bus.deny} disabled={busy} onClick={() => void answer(question.id, false)} />
                    </div>
                </div>
            ))}
            {view.asked.length > 0 && <span class="t-caption">{tr.bus.asksInManifest(view.asked.map((right) => askedWords(right, titles)).join(', '))}</span>}
            {view.links.length > 0 ? (
                view.links.map((link) => <LinkRow key={`${link.from}>${link.to}`} link={link} me={me} titles={titles} onChanged={() => setNonce((n) => n + 1)} />)
            ) : (
                <span class="t-caption">{tr.bus.noOwnLinks}</span>
            )}
            {reaches.length > 0 && (
                <span class="t-caption">
                    {tr.bus.seesNow(
                        reaches
                            .map((visible) => tr.bus.seesItem(visible.title, !visible.handles.some((handle) => handle.allowed && handle.level !== 'see')))
                            .join(', '),
                    )}
                </span>
            )}
            <div class="flex flex-wrap gap-8">
                <Action label={journal ? tr.bus.hideJournal : tr.bus.journalTitle} onClick={() => setJournal((open) => !open)} />
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
