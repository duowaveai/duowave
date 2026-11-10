// Плашка «Делаю виджет» (docs/plans/desk.md; просьба владельца 10.11.2026): пока агент мастерской работает над
// черновиком — по просьбе голосом у колонки, в мастерской или из «+», — в верхней строке стола (в боковой панели — над
// разделами) видно, что работа идёт и чем агент занят сейчас: «пишет panel.js», «проверяет», «думает 23 с». Нажатие
// открывает мастерскую на этом черновике, там — весь ход работы. Закончил — «Готово», «Не вышло: …» или «Остановлено»
// ещё полторы минуты. Работа идёт и без открытого пульта (gateway/src/plugins/workshop/activity.rs); плашка видна тому,
// кто может мастерскую (plugins.manage). У помощника виджет, заказанный голосом у колонки, делает мастерская колонки:
// плашка читает и её, через проход, а нажатие открывает черновик в окне колонки «Плагины».

import { useEffect, useRef, useState } from 'preact/hooks';

import { AgentActivity, AgentJob, AgentStep, ApiError, api, can, problemOf, stationCall, stationRead } from '../api';
import { onStationChange } from '../live';
import { usePoll } from '../poll';
import { ask } from '../ui/confirm';
import { VIA } from '../via';
import { usePanel } from './context';

/** Закончившая работа видна на плашке столько — как её отдаёт шлюз (activity.rs, SHOWN_AFTER_MS). */
const SHOWN_AFTER_MS = 90_000;
/** Пока агент работает — перечитывать так часто, даже если живой канал молчит. */
export const POLL_MS = 2000;

/** Сколько прошло: «23 с», «1 мин 5 с», «1 ч 2 мин». */
export function elapsed(ms: number): string {
    const seconds = Math.max(0, Math.floor(ms / 1000));
    if (seconds < 60) return `${seconds} с`;
    const minutes = Math.floor(seconds / 60);
    if (minutes < 60) return `${minutes} мин ${seconds % 60} с`;
    return `${Math.floor(minutes / 60)} ч ${minutes % 60} мин`;
}

/** Строка покороче: причина неудачи на плашке, ответ вызова в ходе работы. Шлюз пишет ошибку вызова как «error: …». */
export function shortText(text: string, limit = 80): string {
    const plain = text.replace(/^error:\s*/i, '').replace(/\s+/g, ' ').trim();
    return plain.length > limit ? `${plain.slice(0, limit - 1).trim()}…` : plain;
}

/** О чём вызов агента, как его пишет шлюз (activity.rs, about): файл, фраза, имя — или первая строка входа. */
export function aboutOf(input: Record<string, unknown>): string {
    for (const key of ['path', 'phrase', 'text', 'name', 'query']) {
        const value = input[key];
        if (typeof value === 'string') return value;
    }
    const first = Object.values(input).find((value): value is string => typeof value === 'string');
    return first ?? '';
}

/** Вызов агента словами: «пишет panel.js», «читает manifest.json», «проверяет», «пробует фразу», «запускает». */
export function toolWords(tool: string | undefined, about = ''): string {
    const what = shortText(about, 48);
    switch (tool) {
        case 'write_file':
            return what ? `пишет ${what}` : 'пишет файл';
        case 'read_file':
            return what ? `читает ${what}` : 'читает файл';
        case 'delete_file':
            return what ? `удаляет ${what}` : 'удаляет файл';
        case 'draft_files':
            return 'смотрит файлы';
        case 'check':
            return 'проверяет';
        case 'try_phrase':
            return 'пробует фразу';
        case 'run_function':
            return what ? `запускает ${what}` : 'запускает';
        case 'plugin_journal':
            return 'читает журнал плагина';
        case 'install':
            return 'ставит';
        case 'docs':
            return 'читает справочник';
        case 'list_drafts':
            return 'смотрит черновики';
        case 'new_draft':
            return 'заводит черновик';
        default:
            return tool ? `зовёт ${tool}` : 'работает';
    }
}

/**
 * Чем агент занят — по последнему шагу: после просьбы и ответа вызова он думает (сколько — от того шага), вызов —
 * словами, сказанное — «говорит». `now` — на тех же часах, что `step.at`.
 */
export function doingWords(step: AgentStep | undefined, now: number): string {
    if (!step) return 'начинает';
    switch (step.kind) {
        case 'tool':
            return toolWords(step.tool, step.text);
        case 'said':
            return 'говорит';
        case 'asked':
        case 'result':
            return `думает ${elapsed(now - step.at)}`;
        default:
            return endWords(step).toLowerCase();
    }
}

/** Как кончилась работа: «Готово», «Не вышло: <почему>», «Остановлено». */
export function endWords(step: AgentStep | undefined): string {
    if (step?.kind === 'failed') return `Не вышло: ${shortText(step.text, 60) || 'ошибка'}`;
    if (step?.kind === 'stopped') return 'Остановлено';
    return 'Готово';
}

/** Что в слотах пульта у манифеста: `contributes.panel.slots`. */
function panelSlots(manifest: string): string[] {
    try {
        const slots = JSON.parse(manifest)?.contributes?.panel?.slots;
        return Array.isArray(slots) ? slots.filter((slot): slot is string => typeof slot === 'string') : [];
    } catch {
        return [];
    }
}

/**
 * Черновики, про которые уже известно, виджет ли это (слот desk.widgets в манифесте), — до перезагрузки страницы.
 * Черновик колонки — под её ключом (`<колонка>/<id>`): у компьютера может быть свой с тем же id.
 */
const widgetDrafts = new Map<string, boolean>();

/**
 * Виджет ли черновик: по манифесту, прочитанному раз (черновик колонки — у неё, через проход). Пока он читается — по
 * имени: `widget-…` делает «+», `golos-vidzhet-…` — голос («сделай виджет»).
 */
function useWidgetDraft(id: string | undefined, station?: string): boolean {
    const guess = !!id && (id.startsWith('widget-') || id.startsWith('golos-vidzhet'));
    const key = id ? (station ? `${station}/${id}` : id) : '';
    const [, rerender] = useState(0);
    useEffect(() => {
        if (!id || widgetDrafts.has(key)) return;
        let alive = true;
        const path = `/api/v1/workshop/drafts/${encodeURIComponent(id)}/files/manifest.json`;
        const manifest = station
            ? stationRead<{ content: string }>(station, path).then((answer) => answer.value)
            : api.readDraftFile(id, 'manifest.json');
        manifest
            .then((answer) => widgetDrafts.set(key, panelSlots(answer.content).includes('desk.widgets')))
            .catch(() => widgetDrafts.set(key, guess))
            .finally(() => alive && rerender((n) => n + 1));
        return () => {
            alive = false;
        };
    }, [key]);
    return id ? (widgetDrafts.get(key) ?? guess) : false;
}

/**
 * Номер перемены пульта (panel.version), но не чаще раза в `ms`: шаги агента идут пачками, и каждый — перемена
 * (`workshop.changed`). Перечитывать раз на пачку, а не на каждый шаг.
 */
export function useSettledVersion(ms = 400): number {
    const panel = usePanel();
    const [settled, setSettled] = useState(panel.version);
    const latest = useRef(panel.version);
    const pending = useRef(0);
    latest.current = panel.version;
    useEffect(() => {
        if (pending.current || settled === panel.version) return;
        pending.current = window.setTimeout(() => {
            pending.current = 0;
            setSettled(latest.current);
        }, ms);
    }, [panel.version]);
    useEffect(() => () => window.clearTimeout(pending.current), []);
    return settled;
}

/** Часы, что идут раз в секунду, пока `on`: «думает 23 с» и сколько идёт работа — без запросов. */
export function useSecondTick(on: boolean): number {
    const [now, setNow] = useState(() => Date.now());
    useEffect(() => {
        if (!on) return;
        setNow(Date.now());
        const timer = window.setInterval(() => setNow(Date.now()), 1000);
        return () => window.clearInterval(timer);
    }, [on]);
    return now;
}

/** Вопрос перед «Остановить»: работа агента над черновиком прервётся. true — останавливать. */
export function askToStop(name: string): Promise<boolean> {
    return ask({
        title: 'Остановить агента?',
        text: `Работа над «${name}» прервётся. Что уже сделано, останется в черновике.`,
        ok: 'Остановить',
    });
}

/** Чья работа: этого шлюза или колонки помощника (её мастерская — через проход). */
export type Whose = 'own' | 'station';

/** Работа на плашке: чья и на чьих часах — часы её шлюза минус наши, мс. */
export interface PlaqueJob {
    job: AgentJob;
    whose: Whose;
    offset: number;
}

/** Работа одного шлюза, как её прочитали, и сдвиг его часов от наших. */
interface Activity {
    jobs: AgentJob[];
    offset: number;
}

/** Часы шлюза минус наши — по `now` его ответа (`at` — когда ответ пришёл); без `now` (шлюз постарше) — запасной сдвиг. */
export function clockOffset(now: number | undefined, fallback: number, at = Date.now()): number {
    return typeof now === 'number' && Number.isFinite(now) ? now - at : fallback;
}

/**
 * Работы, что видны на плашке в `at` (наши часы): идут или кончились меньше полутора минут назад — по часам своего
 * шлюза. Новые сначала — по началу работы на наших часах: у компьютера и колонки часы свои.
 */
export function plaqueJobs(entries: PlaqueJob[], at: number): PlaqueJob[] {
    return entries
        .filter(({ job, offset }) => job.busy || (job.ended !== undefined && at + offset - job.ended < SHOWN_AFTER_MS))
        .sort((a, b) => b.job.since - b.offset - (a.job.since - a.offset));
}

/**
 * Слова плашки о работе `current`: что делается (или чем кончилось), чем агент занят и сколько ещё идёт (`others` —
 * другие идущие работы). Когда рядом работы и компьютера, и колонки, сказано, где какая. `at` — наши часы.
 */
export function plaqueWords(
    current: PlaqueJob,
    others: PlaqueJob[],
    widget: boolean,
    at: number,
): { what: string; doing: string; tail: string | null } {
    const { job } = current;
    const name = job.title?.trim() || job.draft;
    const all = [current, ...others];
    const mixed = all.some((entry) => entry.whose === 'station') && all.some((entry) => entry.whose === 'own');
    const label = `${widget ? 'виджет ' : ''}«${name}»${mixed && current.whose === 'station' ? ' на колонке' : ''}`;
    let tail: string | null = null;
    if (others.length > 0 && !mixed) tail = `и ещё ${others.length}`;
    else if (others.length > 0) {
        const own = others.filter((entry) => entry.whose === 'own').length;
        const there = others.length - own;
        tail = `и ещё ${[own ? `${own} на компьютере` : '', there ? `${there} на колонке` : ''].filter(Boolean).join(' и ')}`;
    }
    return {
        what: job.busy ? `Делаю ${label}` : endWords(job.last),
        doing: job.busy ? doingWords(job.last, at + current.offset) : label,
        tail,
    };
}

/**
 * Плашка «Делаю виджет»: в верхней строке стола (`top`) — тихая капсула, в боковой панели (`side`) — блок над
 * разделами. Новейшая идущая работа, иначе закончившаяся за последние полторы минуты; «и ещё N» — сколько ещё идёт.
 * У помощника — работы и своей мастерской, и мастерской колонки, вместе.
 */
export function AgentPlaque({ place }: { place: 'top' | 'side' }) {
    const panel = usePanel();
    const allowed = can('plugins.manage');
    // Колонка помощника: что попросили голосом у неё, делает её мастерская. В окне колонки (VIA) плашки нет, а у шлюза
    // колонка — он сам.
    const station = !VIA && panel.role.role === 'helper' ? panel.role.station?.key : undefined;
    const [own, setOwn] = useState<Activity | null>(null);
    const [there, setThere] = useState<(Activity & { station: string }) | null>(null);
    /** Шлюз не отдаёт работу агента этому входу (не администратор): больше не спрашивать. */
    const [refused, setRefused] = useState(false);
    /** Колонка не отдаёт работу своей мастерской: её больше не спрашивать, пока колонка та же. */
    const [stationRefused, setStationRefused] = useState<string | null>(null);
    const [trouble, setTrouble] = useState<string | null>(null);
    const [stopping, setStopping] = useState(false);
    // Запасные часы своего шлюза, если он не сказал `now`: по его живому каналу.
    const skew = useRef(panel.feed.skew);
    skew.current = panel.feed.skew;
    const version = useSettledVersion(250);
    /** Колонка, чью мастерскую читать; null — не читать. */
    const asked = allowed && !refused && station && stationRefused !== station ? station : null;
    /** Перемены мастерской колонки: их приносит её канал, который пересказывает шлюз компьютера (live.ts). */
    const [thereVersion, setThereVersion] = useState(0);
    const busy = !!own?.jobs.some((job) => job.busy);
    const busyThere = !!there?.jobs.some((job) => job.busy);

    // Своя мастерская: сразу и с каждой переменой (каждый шаг агента — `workshop.changed`), пока агент работает — ещё
    // и раз в две секунды: живой канал мог оборваться. Без наложений: пока идёт чтение, нового нет (poll.ts).
    const read = usePoll({
        on: allowed && !refused,
        every: busy ? POLL_MS : 0,
        version,
        settle: 250,
        read: (signal) =>
            api
                .workshopActivity(signal)
                .then((answer) => !signal.aborted && setOwn({ jobs: answer.jobs, offset: clockOffset(answer.now, skew.current) }))
                .catch((e) => {
                    if (!signal.aborted && e instanceof ApiError && (e.status === 401 || e.status === 403)) setRefused(true);
                }),
    });
    // Мастерская колонки — через проход: сразу, по переменам её мастерской (шаги агента идут пачками — раз на пачку) и,
    // пока там идёт работа, раз в две секунды. Другая колонка или её больше нет — прежнее не показывать, идущее чтение
    // оборвать.
    useEffect(() => {
        setThere(null);
        if (!asked) return;
        return onStationChange((name, from) => {
            if (from === asked && (name === 'workshop.changed' || name === 'hello')) setThereVersion((n) => n + 1);
        });
    }, [asked]);
    const readThere = usePoll({
        on: !!asked,
        deps: [asked],
        every: busyThere ? POLL_MS : 0,
        version: thereVersion,
        settle: 250,
        read: (signal) => {
            const key = asked ?? '';
            return stationRead<AgentActivity>(key, '/api/v1/workshop/activity', signal)
                .then((answer) => {
                    if (signal.aborted) return;
                    // Копия: колонки нет на связи — что там идёт сейчас, неизвестно, а прежнее уже не верно. Время — по
                    // часам колонки (`now`), без них — по нашим.
                    const live = answer.copyAt === null ? answer.value : null;
                    setThere(live ? { station: key, jobs: live.jobs, offset: clockOffset(live.now, 0) } : null);
                })
                .catch((e) => {
                    if (signal.aborted || !(e instanceof ApiError)) return;
                    if (e.status === 401 || e.status === 403) setStationRefused(key);
                    // Колонки нет на связи и копии нет, её нет у компьютера, нет пары — не показывать. Сбой по дороге — как было.
                    if (e.status < 500 || e.status === 503) setThere(null);
                });
        },
    });
    // «Не остановить: …» видно несколько секунд, потом плашка снова говорит, чем занят агент.
    useEffect(() => {
        if (!trouble) return;
        const timer = window.setTimeout(() => setTrouble(null), 6000);
        return () => window.clearTimeout(timer);
    }, [trouble]);

    const entries: PlaqueJob[] = [
        ...(own ? own.jobs.map((job) => ({ job, whose: 'own' as const, offset: own.offset })) : []),
        ...(there && there.station === asked ? there.jobs.map((job) => ({ job, whose: 'station' as const, offset: there.offset })) : []),
    ];
    // Время шагов — по часам их шлюза (`now` его ответа): у компьютера и колонки часы свои.
    const recent = plaqueJobs(entries, Date.now());
    const tick = useSecondTick(recent.length > 0);
    const shown = plaqueJobs(recent, tick);
    const current = shown.find((entry) => entry.job.busy) ?? shown[0];
    const widget = useWidgetDraft(current?.job.draft, current?.whose === 'station' ? (asked ?? undefined) : undefined);
    if (!allowed || refused || !current) return null;

    const { job, whose } = current;
    const words = plaqueWords(current, shown.filter((entry) => entry.job.busy && entry !== current), widget, tick);
    const doing = trouble ?? words.doing;
    const end = job.last?.kind;
    const state = job.busy ? 'is-busy' : end === 'failed' ? 'is-failed' : end === 'stopped' ? 'is-stopped' : 'is-done';
    // Черновик колонки — в её окне «Плагины», на вкладке мастерской (StationFrame кладёт его в адрес фрейма).
    const open = () =>
        whose === 'station' ? panel.go('plugins', 'workshop', job.draft, { on: 'station' }) : panel.go('plugins', 'workshop', job.draft);
    const title = whose === 'station' ? 'Открыть в мастерской колонки' : 'Открыть в мастерской';
    const stop = async () => {
        const key = asked;
        if (whose === 'station' && !key) return;
        if (!(await askToStop(job.title?.trim() || job.draft))) return;
        setStopping(true);
        try {
            if (whose === 'station' && key) {
                await stationCall<{ stopped: boolean }>(key, 'POST', `/api/v1/workshop/drafts/${encodeURIComponent(job.draft)}/stop`, {});
                readThere();
            } else {
                await api.stopDraftAgent(job.draft);
                read();
            }
        } catch (e) {
            setTrouble(`не остановить: ${problemOf(e)}`);
        } finally {
            setStopping(false);
        }
    };
    const mark = job.busy ? <span class="agent-spin" aria-hidden="true" /> : <span class={`agent-dot ${state}`} aria-hidden="true" />;
    const stopButton = job.busy && (
        <button class="agent-plaque-stop" disabled={stopping} onClick={() => void stop()}>
            {stopping ? 'Останавливаю…' : 'Остановить'}
        </button>
    );

    if (place === 'side')
        return (
            <div class={`agent-plaque agent-plaque-side ${state}`}>
                <button class="agent-plaque-open" title={title} onClick={open}>
                    {mark}
                    <span class="agent-plaque-lines">
                        <span class="agent-plaque-what">{words.what}</span>
                        <span class="agent-plaque-doing">{[doing, words.tail].filter(Boolean).join(' · ')}</span>
                    </span>
                </button>
                {stopButton}
            </div>
        );
    return (
        <div class={`agent-plaque ${state}`}>
            <button class="agent-plaque-open" title={title} onClick={open}>
                {mark}
                <span class="agent-plaque-text">
                    <span class="agent-plaque-what">{words.what}</span>
                    <span class="agent-plaque-doing"> · {doing}</span>
                    {words.tail && <span class="agent-plaque-doing"> · {words.tail}</span>}
                </span>
            </button>
            {stopButton}
        </div>
    );
}
