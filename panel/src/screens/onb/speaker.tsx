// Первые шаги → «Колонка» для «Колонка + компьютер» и «Только колонка» — доски Check, Plan, Connecting (и *Only)
// холста «Онбординг» 1:1, ошибки — по доске Problems. Три подэкрана одного шага: «Проверим колонку» (только чтение,
// api.checkStation) → «Что изменится» (строки из настоящей проверки, кнопка «Подключить») → «Подключаю колонку»:
// подключение мастера (api.connectStation + connectJob), сразу за ним — перенос DuoWave на колонку (api.startMove +
// moveJob: stationServer или station), строками в том же списке, без второго выбора места.
// «Ждать не нужно»: подключение и перенос живут в этом модуле, а не в экране — человек уходит на «Кто отвечает»,
// а опрос идёт дальше; там же видно, пришла ли колонка (useSpeakerLink). Не пришла за 3 минуты — «Подождать ещё»
// или «Вернуть как было» (api.restoreStation). Шапку и кнопки внизу рисует рамка (onb/nav.tsx).

import { useEffect, useRef, useState } from 'preact/hooks';
import type { ComponentChildren } from 'preact';

import { ApiError, ConnectJob, FoundStation, MoveJob, MoveTo, StationCheck, StationNeighbour, api, can, problemOf } from '../../api';
import { tr } from '../../i18n';
import { drawingOf } from '../../stations';
import { encodeQr } from '../../ui/qr';
import { WIKI_SSH, problemText, suggestedGateway } from '../connect';
import { useOnboarding, useStepNav } from './nav';
import '../../ui/onboarding-canvas.css';
import '../../ui/onb-connect.css';
import { LinkMark, patchRecord, readRecord } from './record';

/** Сколько места нужно DuoWave на колонке (доска: «около 20 МБ»), КБ. */
const ROOM_KB = 20 * 1024;
/** Порт шлюза на колонке, пока перенос не назвал свой (как у «Связать вручную» в placement.tsx). */
const STATION_PORT = 8790;
/** «Подождать ещё»: столько ещё ждём колонку после 3 минут шлюза. */
const WAIT_MORE_MS = 3 * 60_000;

// ─── Подключение и перенос: живут вне экрана ──────────────────────────

type Phase = 'connecting' | 'moving' | 'waiting' | 'timeout' | 'failed' | 'restoring' | 'restored' | 'done';

/** Подключение колонки в первых шагах: что идёт сейчас и чем кончилось. */
export interface SpeakerLink {
    address: string;
    /** Имя колонки, как его знает сеть; пусто — без имени. */
    name: string;
    /** Куда переезжает DuoWave; null — остаётся на компьютере (на колонке мало места). */
    to: MoveTo | null;
    phase: Phase;
    connect: ConnectJob | null;
    move: MoveJob | null;
    restore: ConnectJob | null;
    /** Колонка была подключена к этому DuoWave до этих шагов: связь не трогали. */
    wasConnected: boolean;
    /** Колонка пришла уже после «Подождать ещё». */
    arrivedLate: boolean;
    /** Перенести не вышло: DuoWave остался на компьютере, колонка подключена к нему. */
    moveError?: string;
    /** Подключение или откат не удались — словами. */
    error?: string;
    /** Пульт колонки после переноса. */
    panelUrl?: string;
    startedAt: number;
    waitUntil?: number;
    /** Сколько раз перенос начинали заново, потеряв его задачу (перезапуск шлюза, п. 62). */
    retries?: number;
}

/** Задачу переноса потеряли (шлюз перезапустился) — начать заново столько раз, дальше «перенос прервался». */
const MOVE_RETRIES = 2;

let link: SpeakerLink | null = null;
/** Поколение: новое подключение отменяет ответы старого опроса. */
let generation = 0;
let timer: number | undefined;
const listeners = new Set<() => void>();

function notify() {
    listeners.forEach((listener) => listener());
}

function update(at: number, patch: Partial<SpeakerLink>) {
    if (!link || at !== generation) return;
    link = { ...link, ...patch };
    keepMark(link);
    notify();
}

/**
 * Метка начатого подключения — в памяти вкладки (п. 62): шлюз перезапустился, пульт перезагрузился, и ход терялся —
 * перенос не начинался, пока снова не открыть шаг «Колонка». С меткой ход продолжается сам (`resumeLink`). Кончилось
 * (готово, вернули как было) — метки нет.
 */
const MARK = 'onb-speaker-link';
/** Метка старше — не продолжаем: это было давно, не в этих шагах. */
const MARK_MS = 30 * 60_000;

function keepMark(now: SpeakerLink | null): void {
    const going = now && now.to && now.phase !== 'done' && now.phase !== 'restored' && now.phase !== 'failed';
    const mark: LinkMark | null = going ? { address: now.address, name: now.name, to: now.to as LinkMark['to'], at: now.startedAt } : null;
    // В запись первых шагов (замечание 10 теста 19.11): ход переживает и перезапуск приложения, не только перезагрузку.
    patchRecord({ link: mark });
    try {
        if (mark) sessionStorage.setItem(MARK, JSON.stringify(mark));
        else sessionStorage.removeItem(MARK);
    } catch {
        // приватное окно — продолжить после перезагрузки не выйдет, только и всего
    }
}

/**
 * Продолжить подключение после перезагрузки пульта (п. 62): перенос уже идёт на шлюзе — смотреть его; DuoWave уже на
 * колонке — готово; колонка на связи, а переноса нет — начать его. Метки нет или ход и так идёт — ничего.
 */
export async function resumeLink(): Promise<void> {
    if (link) return;
    let mark: { address: string; name: string; to: MoveTo; at: number } | null = readRecord()?.link ?? null;
    try {
        mark ??= JSON.parse(sessionStorage.getItem(MARK) ?? 'null');
    } catch {
        mark = null;
    }
    if (!mark || Date.now() - mark.at > MARK_MS) return;
    const view = await api.placement().catch(() => null);
    if (!view || link) return;
    if (view.job?.status === 'running') return adoptMove(mark.address, mark.name, view.job);
    const record = view.station?.record;
    if (record && record.placement === mark.to) {
        adoptDone(mark.address, mark.name, mark.to, record.port);
        keepMark(null);
        return;
    }
    await moveNow(mark.to);
}

function schedule(at: number, ms: number) {
    if (at !== generation) return;
    window.clearTimeout(timer);
    timer = window.setTimeout(() => void tick(at), ms);
}

/** Подключение колонки первых шагов — для экранов «Колонка» и «Кто отвечает». null — не начиналось. */
export function useSpeakerLink(): SpeakerLink | null {
    const [, setTick] = useState(0);
    useEffect(() => {
        const listener = () => setTick((n) => n + 1);
        listeners.add(listener);
        return () => void listeners.delete(listener);
    }, []);
    return link;
}

/** Колонка ещё в пути: подключается, переезжает или её ждут. */
export function linkActive(state: SpeakerLink | null): boolean {
    return !!state && (state.phase === 'connecting' || state.phase === 'moving' || state.phase === 'waiting' || state.phase === 'restoring');
}

function panelUrlOf(address: string, port?: number): string {
    return `http://${address}:${port ?? STATION_PORT}/panel`;
}

/** Перенос DuoWave на колонку после подключения; места нет (to = null) — готово сразу. */
async function startMove(at: number) {
    const now = link;
    if (!now || at !== generation) return;
    if (!now.to) return update(at, { phase: 'done' });
    update(at, { phase: 'moving' });
    try {
        const view = await api.placement();
        const record = view.station?.record;
        if (record && record.placement === now.to) return update(at, { phase: 'done', panelUrl: panelUrlOf(record.address, record.port) });
        const job = view.job?.status === 'running' ? view.job : await api.startMove({ to: now.to, address: now.address, settingsHome: 'station' });
        update(at, { move: job });
        schedule(at, 1500);
    } catch (e) {
        update(at, { phase: 'done', moveError: problemOf(e) });
    }
}

async function tick(at: number) {
    const now = link;
    if (!now || at !== generation) return;
    try {
        if (now.phase === 'connecting' && now.connect) {
            let job: ConnectJob;
            try {
                job = await api.connectJob(now.connect.id);
            } catch (e) {
                // Шлюз перезапустился посреди подключения (п. 61: «Кто отвечает» в 12:27:00) — его задача пропала, и опрос
                // крутил её вечно, а до переноса не доходило. Колонку теперь ждём по её связи: пришла — перенос.
                if (e instanceof ApiError && e.status === 404) {
                    update(at, { phase: 'waiting', waitUntil: Date.now() + WAIT_MORE_MS });
                    return schedule(at, 1000);
                }
                throw e;
            }
            if (job.status === 'running') {
                update(at, { connect: job });
                return schedule(at, 1200);
            }
            if (job.status === 'done') {
                update(at, { connect: job });
                return void startMove(at);
            }
            if (job.status === 'timeout') return update(at, { connect: job, phase: 'timeout' });
            return update(at, { connect: job, phase: 'failed', error: problemText(job.error) + (job.copy ? tr.connect.ourCopy(job.copy) : '') });
        }
        if (now.phase === 'moving' && now.move) {
            let job: MoveJob;
            try {
                job = await api.moveJob(now.move.id);
            } catch (e) {
                // Та же пропажа задачи переноса: где DuoWave сейчас — по записи шлюза.
                // Не перенесён — ждём колонку на связи с этим шлюзом и начинаем перенос заново (п. 62): он сам доходит до
                // конца, нажимать «Перенести сейчас» не нужно. Два раза не вышло — говорим, что прервался.
                if (e instanceof ApiError && e.status === 404) {
                    const view = await api.placement().catch(() => null);
                    const record = view?.station?.record;
                    if (record && record.placement === now.to) return update(at, { phase: 'done', panelUrl: panelUrlOf(record.address, record.port) });
                    if (view?.job?.status === 'running') {
                        update(at, { move: view.job });
                        return schedule(at, 1500);
                    }
                    const retries = (now.retries ?? 0) + 1;
                    if (retries > MOVE_RETRIES) return update(at, { phase: 'done', moveError: tr.onbCharacter.moveLost });
                    update(at, { phase: 'waiting', move: null, retries, startedAt: Date.now(), waitUntil: Date.now() + WAIT_MORE_MS });
                    return schedule(at, 1000);
                }
                throw e;
            }
            update(at, { move: job });
            if (job.status === 'running') return schedule(at, 1500);
            if (job.status === 'failed') return update(at, { phase: 'done', moveError: job.error ?? '' });
            const view = await api.placement().catch(() => null);
            const record = view?.station?.record;
            return update(at, { phase: 'done', panelUrl: panelUrlOf(record?.address ?? now.address, record?.port) });
        }
        if (now.phase === 'waiting') {
            const stations = await api.stations();
            const came = stations.some((station) => station.lastAddress === now.address && Date.parse(station.lastSeen) >= now.startedAt);
            if (came) {
                if (!now.retries) update(at, { arrivedLate: true });
                return void startMove(at);
            }
            if (Date.now() > (now.waitUntil ?? 0)) return update(at, { phase: 'timeout' });
            return schedule(at, 5000);
        }
        if (now.phase === 'restoring' && now.restore) {
            const job = await api.connectJob(now.restore.id);
            update(at, { restore: job });
            if (job.status === 'running') return schedule(at, 1200);
            if (job.status === 'done') return update(at, { phase: 'restored' });
            return update(at, { phase: 'failed', error: problemText(job.error) });
        }
    } catch {
        // Связь со шлюзом мигнула — спросим ещё раз.
        schedule(at, 2500);
    }
}

/** «Подключить»: подключение мастера, за ним — перенос. Ошибка старта — наружу, рамка покажет её строкой. */
async function startLink(input: { address: string; name: string; gateway: string; to: MoveTo | null; connected: boolean }) {
    const at = ++generation;
    window.clearTimeout(timer);
    link = {
        address: input.address,
        name: input.name,
        to: input.to,
        phase: 'connecting',
        connect: null,
        move: null,
        restore: null,
        wasConnected: input.connected,
        arrivedLate: false,
        startedAt: Date.now(),
    };
    keepMark(link);
    notify();
    if (input.connected) return void startMove(at);
    try {
        const job = await api.connectStation(input.address, input.gateway);
        update(at, { connect: job });
        schedule(at, 1200);
    } catch (e) {
        link = null;
        notify();
        throw e;
    }
}

/** DuoWave уже на колонке (вернулись на шаг или пульт перезагрузили): сразу «готово». */
function adoptDone(address: string, name: string, to: MoveTo, port?: number) {
    generation++;
    link = {
        address,
        name,
        to,
        phase: 'done',
        connect: null,
        move: null,
        restore: null,
        wasConnected: true,
        arrivedLate: false,
        startedAt: Date.now(),
        panelUrl: panelUrlOf(address, port),
    };
    notify();
}

/**
 * Что сейчас идёт, словами — для шага «Персонаж» (п. 62): «голос колонки — к DuoWave (перезагрузка, около минуты)».
 * Те же строки, что у доски шага «Колонка». Ничего не идёт — null.
 */
export function linkNow(state: SpeakerLink | null): string | null {
    if (!state || !linkActive(state)) return null;
    const t = tr.onbConnect;
    const say = (title: string, sub: string) => {
        // «Голос колонки» → «голос колонки», а имя («DuoWave», в первом слове есть ещё заглавная) — как есть (п. 63).
        const head = /^\p{Lu}\p{Ll}*(?=[\s,—]|$)/u.test(title) ? title[0].toLocaleLowerCase() + title.slice(1) : title;
        return `${head} (${sub})`;
    };
    if (state.phase === 'restoring') return t.restoring;
    if (state.phase === 'waiting') return t.rebooting;
    if (state.phase === 'moving') {
        const step = state.move?.steps.find((one) => one.state === 'running')?.id;
        if (step === 'settings' || step === 'point') return say(t.stepVoice, t.stepVoiceSub);
        if (step === 'arrive') return t.rebooting;
        return say(t.stepInstall, t.stepInstallSub);
    }
    const step = state.connect?.steps.find((one) => one.state === 'running')?.id;
    if (step === 'copy' || step === 'remove') return say(t.stepBack, t.stepBackSub);
    if (step === 'reboot' || step === 'arrive') return t.rebooting;
    return state.to ? say(t.stepLink, t.stepLinkSub) : say(t.stepVoicePc, t.stepVoicePcSub);
}

/** Перенос уже идёт на шлюзе — смотрим его. */
function adoptMove(address: string, name: string, job: MoveJob) {
    const at = ++generation;
    link = { address, name, to: job.to, phase: 'moving', connect: null, move: job, restore: null, wasConnected: true, arrivedLate: false, startedAt: Date.now() };
    notify();
    schedule(at, 1500);
}

/**
 * «Перенести сейчас» с шага «Персонаж» (п. 61): колонка подключена и на связи, а перенос так и не начался. Начать его
 * тем же путём, что после «Подключить». Колонки на связи нет — false.
 */
export async function moveNow(to: MoveTo): Promise<boolean> {
    const stations = await api.stations();
    const fresh = stations
        .filter((station) => station.status === 'approved' && station.lastAddress && Date.now() - Date.parse(station.lastSeen) < 2 * 60_000)
        .sort((a, b) => Date.parse(b.lastSeen) - Date.parse(a.lastSeen))[0];
    if (!fresh?.lastAddress) return false;
    const at = ++generation;
    window.clearTimeout(timer);
    link = {
        address: fresh.lastAddress,
        name: fresh.name,
        to,
        phase: 'moving',
        connect: null,
        move: null,
        restore: null,
        wasConnected: true,
        arrivedLate: false,
        startedAt: Date.now(),
    };
    notify();
    void startMove(at);
    return true;
}

/** «Подождать ещё»: ещё 3 минуты смотрим, не пришла ли колонка. */
export function waitMore() {
    if (!link) return;
    update(generation, { phase: 'waiting', waitUntil: Date.now() + WAIT_MORE_MS });
    schedule(generation, 1000);
}

/** «Вернуть как было»: прежние адреса голосового сервера и перезагрузка. */
export async function restoreLink() {
    const now = link;
    if (!now) return;
    const at = generation;
    window.clearTimeout(timer);
    const job = await api.restoreStation(now.address, 'previous');
    update(at, { phase: 'restoring', restore: job, error: undefined });
    schedule(at, 1200);
}

function clearLink() {
    generation++;
    window.clearTimeout(timer);
    link = null;
    notify();
}

// ─── QR пульта колонки ───────────────────────────────────────────────

/** QR адреса пульта колонки — тем же кодировщиком, что у «Подключить шлюз» (ui/qr.ts): кодировщик QR в пульте один. */
function Qr({ text, label }: { text: string; label: string }) {
    const code = encodeQr(text);
    if (!code) return null;
    return (
        <div class="qr onb-cx-qr" aria-label={label} role="img" style={{ gridTemplateColumns: `repeat(${code.size}, 3px)` }}>
            {Array.from(code.modules, (on, at) => <i key={at} class={on ? 'b' : ''} />)}
        </div>
    );
}

// ─── Общие кусочки досок ─────────────────────────────────────────────

const IconOk = () => (
    <svg class="ico ico-sm" viewBox="0 0 24 24" aria-hidden="true">
        <path d="M5 12.5l4.5 4.5L19 7.5" />
    </svg>
);
const IconWarn = () => (
    <svg class="ico ico-sm" viewBox="0 0 24 24" aria-hidden="true">
        <path d="M12 7v6M12 16.5v.5" />
    </svg>
);

type Tone = 'ok' | 'warn' | 'bad' | 'busy' | 'wait';

function Mark({ tone, children }: { tone: Tone; children?: ComponentChildren }) {
    return (
        <span class={`mark mark-${tone}`}>
            {children ?? (tone === 'ok' ? <IconOk /> : tone === 'warn' || tone === 'bad' ? <IconWarn /> : null)}
        </span>
    );
}

interface Row {
    title: string;
    sub: ComponentChildren;
    tone: Tone;
    bar?: boolean;
}

function Checks({ rows, wide = false }: { rows: Row[]; wide?: boolean }) {
    return (
        <ul class="checks">
            {rows.map((row, at) => (
                <li key={at}>
                    <Mark tone={row.tone} />
                    <span style={wide ? { flex: 1 } : undefined}>
                        <span class="t">{row.title}</span>
                        <br />
                        <span class="s">{row.sub}</span>
                        {row.bar && (
                            <span class="bar bar-run" style={{ display: 'block', marginTop: '8px' }}>
                                <i />
                            </span>
                        )}
                    </span>
                </li>
            ))}
        </ul>
    );
}

/** Карточка доски Problems: знак, заголовок, текст и кнопки. */
function Problem({ tone, icon, title, children }: { tone: Tone; icon?: ComponentChildren; title: string; children: ComponentChildren }) {
    return (
        <section class="card" style={{ display: 'flex', flexDirection: 'column', gap: '10px', padding: '18px 20px' }}>
            <span style={{ display: 'flex', alignItems: 'center', gap: '10px' }}>
                <Mark tone={tone}>{icon}</Mark>
                <span class="h3">{title}</span>
            </span>
            {children}
        </section>
    );
}

const IconSearch = () => (
    <svg class="ico ico-sm" viewBox="0 0 24 24" aria-hidden="true">
        <circle cx="11" cy="11" r="7" />
        <path d="M20 20l-4-4" />
    </svg>
);
const IconBox = () => (
    <svg class="ico ico-sm" viewBox="0 0 24 24" aria-hidden="true">
        <rect x="4" y="4" width="16" height="16" rx="3" />
        <path d="M8 15h8" />
    </svg>
);
const IconClock = () => (
    <svg class="ico ico-sm" viewBox="0 0 24 24" aria-hidden="true">
        <circle cx="12" cy="12" r="8" />
        <path d="M12 8v4l3 2" />
    </svg>
);
const IconLaptop = ({ size = 'sm' }: { size?: 'sm' | 'lg' }) => (
    <svg class={`ico ico-${size}`} viewBox="0 0 24 24" aria-hidden="true">
        <rect x="3" y="5" width="18" height="11" rx="2" />
        <path d="M2 19h20" />
    </svg>
);
const IconPhone = () => (
    <svg class="ico ico-sm" viewBox="0 0 24 24" aria-hidden="true">
        <rect x="7" y="2.5" width="10" height="19" rx="2.5" />
        <path d="M11 18.5h2" />
    </svg>
);

const DIVI_BODY = 'M116 40H140A68 68 0 0 1 208 108V148A68 68 0 0 1 140 216H116A68 68 0 0 1 48 148V108A68 68 0 0 1 116 40Z';

/** Тон группы шагов задания: что-то упало — плохо, всё сделано — готово, что-то идёт — идёт. */
function toneOf(steps: Array<{ id: string; state: string }> | undefined, ids: string[], finished: boolean): Tone {
    if (!steps) return 'wait';
    const mine = steps.filter((step) => ids.includes(step.id));
    if (!mine.length) return finished ? 'ok' : 'wait';
    if (mine.some((step) => step.state === 'failed')) return 'bad';
    if (mine.every((step) => step.state === 'done' || step.state === 'skipped')) return 'ok';
    if (mine.some((step) => step.state !== 'pending')) return 'busy';
    return 'wait';
}

// ─── Шаг «Колонка» ───────────────────────────────────────────────────

type Sub = 'check' | 'plan' | 'connecting';

/** Модель колонки словами: из mDNS или из её настроек. */
function modelOf(station: FoundStation | undefined, check: StationCheck | null): string {
    const code = station?.txt.platform || check?.facts?.deviceType;
    return code ? drawingOf(null, code).title : tr.onbConnect.speakerDefault;
}

/** Рисунок колонки: модель узнали (mDNS или проверка) — её рисунок из «Колонок» пульта, нет — простой силуэт холста. */
function SpeakerArt({ station, check }: { station: FoundStation | undefined; check: StationCheck | null }) {
    const code = station?.txt.platform || check?.facts?.deviceType;
    const drawing = code ? drawingOf(null, code) : null;
    if (!drawing?.drawn)
        return (
            <span class="speaker dim" style={{ width: '44px', height: '60px', borderRadius: '22px', paddingTop: '8px' }}>
                <i style={{ width: '24px', height: '5px' }} />
            </span>
        );
    const { Art } = drawing;
    return (
        <span class="onb-cx-art" aria-hidden="true">
            <Art skin={drawing.color} clock={drawing.clock} sticker={drawing.sticker} ring="var(--bar-own)" stage="idle" />
        </span>
    );
}

/** Строки «Проверим колонку» из настоящей проверки. */
function checkRows(check: StationCheck, model: string): Row[] {
    const t = tr.onbConnect;
    if (check.problem) {
        const title =
            check.problem === 'no-ssh' || check.problem === 'auth-failed'
                ? t.rowNoSsh
                : check.problem === 'no-config'
                  ? t.rowNoConfig
                  : check.problem === 'unreachable' || check.problem === 'station-off'
                    ? t.rowUnreachable
                    : check.problem === 'no-route'
                      ? t.rowNoRoute
                      : check.problem === 'ssh-slow' || check.problem === 'ssh-closed'
                        ? t.rowSshSlow
                        : check.problem === 'host-key-changed'
                          ? t.rowHostKey
                          : t.rowProblem;
        return [{ title, sub: problemText(check.problem), tone: 'bad' }];
    }
    const rows: Row[] = [];
    if (check.connected) rows.push({ title: t.rowConnected, sub: t.rowConnectedSub, tone: 'ok' });
    rows.push({ title: t.rowModel, sub: t.rowModelSub(model), tone: 'ok' });
    const mb = check.dataFreeKb !== undefined ? Math.floor(check.dataFreeKb / 1024) : undefined;
    if (mb === undefined) rows.push({ title: t.rowSpace, sub: t.rowSpaceUnknown, tone: 'ok' });
    else if ((check.dataFreeKb ?? 0) < ROOM_KB) rows.push({ title: t.rowSpaceLow, sub: t.rowSpaceLowSub(mb), tone: 'warn' });
    else rows.push({ title: t.rowSpace, sub: t.rowSpaceSub(mb), tone: 'ok' });
    const memory = check.neighbours?.find((neighbour) => neighbour.id === 'low-memory');
    rows.push(
        memory
            ? { title: t.rowMemoryLow, sub: tr.connect.freeMemory(memory.detail ?? '?'), tone: 'warn' }
            : { title: t.rowMemory, sub: t.rowMemorySub, tone: 'ok' },
    );
    const mods = modsOf(check);
    const known = tr.connect.neighbour as Record<string, { name: string } | undefined>;
    if (!mods.length) rows.push({ title: t.rowMods, sub: t.rowModsSub, tone: 'ok' });
    else {
        const blocks = mods.some((neighbour) => neighbour.severity === 'blocks');
        rows.push({
            title: blocks ? t.rowModsBlock : t.rowModsWarn,
            sub: mods.map((neighbour) => known[neighbour.id]?.name ?? neighbour.id).join(', '),
            tone: blocks ? 'bad' : 'warn',
        });
    }
    if (check.updates === 'on') rows.push({ title: t.rowUpdatesOn, sub: t.rowUpdatesOnSub, tone: 'warn' });
    else if (check.updates === 'blocked') rows.push({ title: t.rowUpdatesOff, sub: t.rowUpdatesOffSub, tone: 'ok' });
    return rows;
}

/** Чужое на колонке, что ещё мешает (кроме нехватки памяти — у неё своя строка). */
function modsOf(check: StationCheck): StationNeighbour[] {
    return (check.neighbours ?? []).filter((neighbour) => neighbour.id !== 'low-memory' && !neighbour.off);
}

/** Пять строк доски, пока проверка идёт: «проверяю…». */
function waitingRows(): Row[] {
    const t = tr.onbConnect;
    return [t.rowModel, t.rowSpace, t.rowMemory, t.rowMods, t.rowUpdatesOff].map((title, at) => ({ title, sub: t.checking, tone: at === 0 ? 'busy' : 'wait' }));
}

/** Шаг «Колонка» первых шагов: проверить → что изменится → подключаю (и перенос). */
export function SpeakerStep() {
    const t = tr.onbConnect;
    const { choice } = useOnboarding();
    const only = choice === 'station';
    const target: MoveTo = only ? 'station' : 'stationServer';
    const live = useSpeakerLink();
    const mayManage = can('stations.manage');

    const [sub, setSub] = useState<Sub>(link ? 'connecting' : 'check');
    const [found, setFound] = useState<FoundStation[] | null>(null);
    const [address, setAddress] = useState('');
    const [typed, setTyped] = useState('');
    const [gateway, setGateway] = useState(suggestedGateway());
    const [check, setCheck] = useState<StationCheck | null>(null);
    const [checking, setChecking] = useState(false);
    const [shown, setShown] = useState(0);
    const [error, setError] = useState<string | null>(null);
    /** Мало места — «Так и сделать»: DuoWave остаётся на компьютере. */
    const [stay, setStay] = useState(false);
    /** «Подключить всё равно» при том, что мешает. */
    const [forced, setForced] = useState(false);
    const [asking, setAsking] = useState<string | null>(null);
    const [turning, setTurning] = useState<string | null>(null);
    const [turnError, setTurnError] = useState<{ id: string; text: string } | null>(null);
    const [busy, setBusy] = useState(false);
    const reads = useRef(0);

    const read = async (at: string, gw = gateway) => {
        const mine = ++reads.current;
        setAddress(at);
        setChecking(true);
        setError(null);
        setCheck(null);
        setShown(0);
        setStay(false);
        setForced(false);
        setAsking(null);
        try {
            const result = await api.checkStation(at, gw.trim() || undefined);
            if (mine === reads.current) setCheck(result);
        } catch (e) {
            if (mine === reads.current) setError(problemOf(e));
        } finally {
            if (mine === reads.current) setChecking(false);
        }
    };

    // Сначала — не переехал ли уже DuoWave (вернулись на шаг, пульт перезагрузили); потом поиск и проверка.
    useEffect(() => {
        if (link) return;
        let alive = true;
        void (async () => {
            try {
                const view = await api.placement();
                const record = view.station?.record;
                const name = view.station?.live.health?.placement?.name ?? '';
                if (!alive) return;
                if (record && record.placement !== 'pc') {
                    adoptDone(record.address, name, record.placement, record.port);
                    setSub('connecting');
                    return;
                }
                if (record && view.job?.status === 'running') {
                    adoptMove(record.address, name, view.job);
                    setSub('connecting');
                    return;
                }
            } catch {
                // Шлюз постарше без размещения — просто ищем колонку.
            }
            if (!mayManage) return void (alive && setFound([]));
            try {
                const result = await api.discoverStations();
                if (!alive) return;
                const gw = gateway || result.gateway?.[0] || '';
                setGateway(gw);
                setFound(result.stations);
                const first = result.stations.find((station) => station.ssh) ?? result.stations[0];
                if (first) void read(first.address, gw);
            } catch {
                if (alive) setFound([]);
            }
        })();
        return () => {
            alive = false;
        };
    }, []);

    const station = found?.find((entry) => entry.address === address);
    const name = station?.name && station.name !== modelOf(station, check) ? station.name : '';
    const model = modelOf(station, check);
    const rows = check ? checkRows(check, model) : waitingRows();

    // Строки проверки появляются по одной, как на доске.
    useEffect(() => {
        if (!check) return;
        setShown(0);
        const id = window.setInterval(() => setShown((n) => (n >= rows.length ? (window.clearInterval(id), n) : n + 1)), 350);
        return () => window.clearInterval(id);
    }, [check]);
    const revealed = !!check && shown >= rows.length;
    const shownRows: Row[] = check
        ? rows.map((row, at) => (at < shown ? row : { ...row, sub: t.checking, tone: at === shown ? 'busy' : 'wait' }))
        : address || checking || found === null
          ? rows
          : [];

    const mods = check && !check.problem ? modsOf(check) : [];
    const blocking = mods.some((neighbour) => neighbour.severity === 'blocks');
    const low = !!check && !check.problem && check.dataFreeKb !== undefined && check.dataFreeKb < ROOM_KB;
    const ready = revealed && !check?.problem && (!blocking || forced) && (!low || stay) && !!gateway.trim() && mayManage;
    const to: MoveTo | null = stay ? null : target;

    const turnOff = async (id: string) => {
        setTurning(id);
        setTurnError(null);
        try {
            await api.turnOffStationMod(address, id);
            setAsking(null);
            const fresh = await api.checkStation(address, gateway.trim() || undefined);
            setCheck(fresh);
        } catch (e) {
            setTurnError({ id, text: problemOf(e) });
        } finally {
            setTurning(null);
        }
    };

    const restore = async () => {
        setBusy(true);
        try {
            await restoreLink();
        } catch (e) {
            setError(problemOf(e));
        } finally {
            setBusy(false);
        }
    };

    // Свежие значения для кнопок рамки: их замыкания живут от смены deps до следующей.
    const latest = useRef({ check, gateway, name, to });
    latest.current = { check, gateway, name, to };

    const phase = live?.phase;
    const finished = phase === 'done' || phase === 'restored';
    useStepNav(
        sub === 'check'
            ? {
                  label: t.next,
                  disabled: !ready,
                  onNext: () => {
                      setSub('plan');
                      return false;
                  },
              }
            : sub === 'plan'
              ? {
                    label: t.connect,
                    disabled: !ready,
                    onNext: async () => {
                        const now = latest.current;
                        if (!now.check) return false;
                        await startLink({ address: now.check.address, name: now.name, gateway: now.gateway.trim(), to: now.to, connected: !!now.check.connected });
                        setSub('connecting');
                        return false;
                    },
                    onBack: () => {
                        setSub('check');
                        return false;
                    },
                }
              : {
                    label: finished ? t.next : t.nextNoWait,
                    hideBack: phase !== 'failed' && phase !== 'restored',
                    onBack: () => {
                        // Не вышло или вернули как было — снова «Проверим колонку», на той же колонке.
                        const at = link?.address;
                        clearLink();
                        setSub('check');
                        if (at) void read(at);
                        return false;
                    },
                },
        [sub, ready, finished, phase],
    );

    if (sub === 'connecting' && live) return <Connecting live={live} only={only} busy={busy} onRestore={() => void restore()} error={error} />;
    if (sub === 'plan' && check) return <Plan check={check} only={only} stay={stay} />;

    // «Проверим колонку» и ошибки доски Problems слева, под карточкой колонки.
    const problem = check?.problem;
    const notFound =
        (found !== null && !found.length && !address) || problem === 'unreachable' || problem === 'station-off' || problem === 'no-route';
    return (
        <div class="onb-cx onb-cx-check">
            <section class="onb-cx-col onb-cx-l">
                <h1 class="h2">{t.checkTitle}</h1>
                <p class="lead onb-cx-lead">{t.checkLead(name)}</p>
                <div class="card card-tight" style={{ display: 'flex', alignItems: 'center', gap: '16px' }}>
                    <SpeakerArt station={station} check={check} />
                    <span>
                        <span class="h3">{found === null && !address ? t.searching : model}</span>
                        <br />
                        {address && <span class="small">{t.speakerLine(name, address)}</span>}
                    </span>
                </div>
                {found && found.length > 1 && (
                    <div class="onb-cx-chips">
                        <span class="small">{t.otherSpeaker}</span>
                        {found.map((entry) => (
                            <button
                                key={entry.address}
                                type="button"
                                class={`chip${entry.address === address ? ' on' : ''}`}
                                aria-pressed={entry.address === address}
                                onClick={() => entry.address !== address && void read(entry.address)}
                            >
                                {entry.address}
                            </button>
                        ))}
                    </div>
                )}
                <p class="small onb-cx-p">{t.sameWifi}</p>
                {!mayManage && <p class="small onb-cx-p">{t.adminOnly}</p>}
                {address && (
                    <button type="button" class="dw-btn dw-btn-light" style={{ alignSelf: 'flex-start' }} disabled={checking} onClick={() => void read(address)}>
                        {t.checkAgain}
                    </button>
                )}

                {notFound && mayManage && (
                    <Problem tone="warn" icon={<IconSearch />} title={t.notFoundTitle}>
                        <p class="small onb-cx-p">{t.notFoundText}</p>
                        <form
                            class="onb-cx-addr"
                            onSubmit={(e) => {
                                e.preventDefault();
                                if (typed.trim()) void read(typed.trim());
                            }}
                        >
                            <label for="onb-cx-addr" style={{ position: 'absolute', left: '-9999px' }}>
                                {t.addressLabel}
                            </label>
                            <input
                                id="onb-cx-addr"
                                class="input"
                                inputMode="decimal"
                                placeholder={t.addressPlaceholder}
                                value={typed}
                                onInput={(e) => setTyped((e.target as HTMLInputElement).value)}
                            />
                            <button type="submit" class="dw-btn dw-btn-dark" disabled={!typed.trim() || checking}>
                                {t.check}
                            </button>
                        </form>
                        <p class="small onb-cx-p">{t.lanNote}</p>
                    </Problem>
                )}

                {(problem === 'no-ssh' || problem === 'auth-failed') && (
                    <Problem tone="bad" title={t.sshTitle}>
                        <p class="small onb-cx-p">{problemText(problem)}</p>
                        <a class="small" href={WIKI_SSH} target="_blank" rel="noreferrer">
                            {t.sshWiki}
                        </a>
                    </Problem>
                )}

                {revealed &&
                    mods.map((neighbour) => {
                        const words = (tr.connect.neighbour as Record<string, { name: string; why: string; back: string } | undefined>)[neighbour.id];
                        const blocks = neighbour.severity === 'blocks';
                        return (
                            <Problem key={neighbour.id} tone={blocks ? 'bad' : 'warn'} title={blocks ? t.modsTitle : t.modsTitleWarn}>
                                <p class="small onb-cx-p">
                                    <span class="q">{words?.name ?? neighbour.id}</span>
                                    {words?.why ? ` — ${words.why}` : neighbour.detail ? ` — ${neighbour.detail}` : ''}
                                </p>
                                {asking === neighbour.id && neighbour.switch && (
                                    <>
                                        <p class="small onb-cx-p">{t.turnOffAsk}</p>
                                        <code class="code-block onb-cx-code">{neighbour.switch}</code>
                                        {words?.back && (
                                            <p class="small onb-cx-p">
                                                {tr.connect.turnBack} <span class="q">{words.back}</span>
                                            </p>
                                        )}
                                    </>
                                )}
                                <div class="onb-cx-btns">
                                    {neighbour.switch &&
                                        (asking === neighbour.id ? (
                                            <button type="button" class="dw-btn dw-btn-dark" disabled={turning !== null} onClick={() => void turnOff(neighbour.id)}>
                                                {turning === neighbour.id ? t.turningOff : t.turnOffYes}
                                            </button>
                                        ) : (
                                            <button type="button" class="dw-btn dw-btn-dark" disabled={turning !== null} onClick={() => setAsking(neighbour.id)}>
                                                {t.turnOff}
                                            </button>
                                        ))}
                                    <button type="button" class="dw-btn dw-btn-light" disabled={checking || turning !== null} onClick={() => void read(address)}>
                                        {t.checkAgain}
                                    </button>
                                    {blocks && !forced && (
                                        <button
                                            type="button"
                                            class="btn-ghost"
                                            onClick={() => {
                                                setForced(true);
                                                if (!low || stay) setSub('plan');
                                            }}
                                        >
                                            {t.connectAnyway}
                                        </button>
                                    )}
                                </div>
                                {turnError?.id === neighbour.id && (
                                    <p role="alert" class="small onb-cx-p onb-cx-bad">
                                        {turnError.text}
                                    </p>
                                )}
                                <p class="small onb-cx-p">{t.modsNote}</p>
                            </Problem>
                        );
                    })}

                {revealed && low && !stay && (
                    <Problem tone="warn" icon={<IconBox />} title={t.lowSpaceTitle}>
                        <p class="small onb-cx-p">{t.lowSpaceText(Math.floor((check?.dataFreeKb ?? 0) / 1024))}</p>
                        <div class="onb-cx-btns">
                            <button
                                type="button"
                                class="dw-btn dw-btn-dark"
                                onClick={() => {
                                    setStay(true);
                                    if (!blocking || forced) setSub('plan');
                                }}
                            >
                                {t.lowSpaceOk}
                            </button>
                        </div>
                    </Problem>
                )}
                {stay && <p class="small onb-cx-p">{t.lowSpaceChosen}</p>}

                {revealed && !problem && !gateway.trim() && (
                    <Problem tone="warn" icon={<IconLaptop />} title={t.gatewayTitle}>
                        <p class="small onb-cx-p">{t.gatewayText}</p>
                        <input
                            class="input"
                            style={{ height: '40px' }}
                            aria-label={t.gatewayLabel}
                            placeholder="192.168.1.10"
                            value={gateway}
                            onInput={(e) => setGateway((e.target as HTMLInputElement).value)}
                        />
                    </Problem>
                )}

                {error && (
                    <p role="alert" class="small onb-cx-p onb-cx-bad">
                        {error}
                    </p>
                )}
            </section>

            {shownRows.length > 0 && (
                <section class="card onb-cx-col onb-cx-r" aria-live="polite">
                    <Checks rows={shownRows} />
                </section>
            )}
        </div>
    );
}

/** «Что изменится на колонке»: пять строк доски из настоящей проверки и как вернуть. */
function Plan({ check, only, stay }: { check: StationCheck; only: boolean; stay: boolean }) {
    const t = tr.onbConnect;
    const facts = check.facts;
    const free = check.dataFreeKb !== undefined ? Math.floor(check.dataFreeKb / 1024) : undefined;
    const voiceWay =
        facts && check.target ? (
            <>
                <br />
                {t.planNow} <span class="q">{facts.addresses.url}</span> → {t.planThen} <span class="q">{check.target.url}</span>
            </>
        ) : null;
    const items: Row[] = [
        stay
            ? { title: t.planStay, sub: t.planStaySub, tone: 'wait' }
            : { title: t.planInstall, sub: free !== undefined ? `${t.planInstallSub} · ${t.rowSpaceSub(free)}` : t.planInstallSub, tone: 'wait' },
        {
            title: stay ? t.planVoicePc : t.planVoice,
            sub: (
                <>
                    {t.planVoiceSub}
                    {voiceWay}
                </>
            ),
            tone: 'wait',
        },
        stay
            ? { title: t.planPc, sub: t.planPcSub, tone: 'wait' }
            : only
              ? { title: t.planOnly, sub: t.planOnlySub, tone: 'wait' }
              : { title: t.planHelper, sub: t.planHelperSub, tone: 'wait' },
        { title: t.planReboot, sub: t.planRebootSub, tone: 'wait' },
        { title: t.planFirst, sub: t.planFirstSub, tone: 'wait' },
    ];
    const warnings = [
        facts?.pointsTo === 'gateway' && !check.connected ? t.otherGateway(facts.addresses.url) : null,
        check.copy?.refused === 'firmware-changed' ? tr.connect.firmwareChanged : null,
        check.copy?.refused === 'gateway-off' ? tr.connect.gatewayOff : null,
    ].filter((line): line is string => !!line);
    return (
        <div class="onb-cx onb-cx-plan">
            <section class="onb-cx-col onb-cx-l">
                <h1 class="h2">{t.planTitle}</h1>
                <p class="lead onb-cx-lead">{t.planLead}</p>
                <div class="card" style={{ padding: '6px 22px' }}>
                    <ul class="checks">
                        {items.map((item, at) => (
                            <li key={at}>
                                <span class="mark" style={{ background: 'var(--chip)', color: 'var(--text)', font: '600 12px/1 var(--font-ui)' }}>
                                    {String(at + 1)}
                                </span>
                                <span>
                                    <span class="t">{item.title}</span>
                                    <br />
                                    <span class="s">{item.sub}</span>
                                </span>
                            </li>
                        ))}
                    </ul>
                </div>
                {warnings.map((line) => (
                    <p key={line} class="small onb-cx-note">
                        <span class="mark mark-warn onb-cx-mark-sm">
                            <IconWarn />
                        </span>
                        {line}
                    </p>
                ))}
            </section>

            <section class="onb-cx-col onb-cx-r">
                <div class="card" style={{ display: 'flex', flexDirection: 'column', gap: '10px', borderColor: 'rgba(101, 163, 13, .45)' }}>
                    <span style={{ display: 'flex', alignItems: 'center', gap: '10px' }}>
                        <span class="mark mark-ok">
                            <svg class="ico ico-sm" viewBox="0 0 24 24" aria-hidden="true">
                                <path d="M9 14l-5-5 5-5" />
                                <path d="M4 9h10a6 6 0 0 1 0 12h-3" />
                            </svg>
                        </span>
                        <span class="h3">{t.backTitle}</span>
                    </span>
                    <p class="small onb-cx-p">{t.backPanel}</p>
                    <p class="small onb-cx-p">{t.backCommand}</p>
                    <code class="code-block onb-cx-code">ssh -p 2222 root@{check.address} sh /data/opt/duowave/restore.sh</code>
                </div>
                {only && !stay && (
                    <div class="card card-tight" style={{ display: 'flex', gap: '12px', alignItems: 'flex-start' }}>
                        <span class="mark mark-ok">
                            <IconPhone />
                        </span>
                        <span class="small">
                            {t.panelMoves} <span class="q">{panelUrlOf(check.address)}</span> {t.panelMovesAfter}
                        </span>
                    </div>
                )}
                {!only && !stay && (
                    <div class="card card-tight" style={{ display: 'flex', gap: '12px', alignItems: 'flex-start' }}>
                        <span class="mark mark-ok">
                            <IconLaptop />
                        </span>
                        <span class="small">{t.panelStays}</span>
                    </div>
                )}
            </section>
        </div>
    );
}

/** «Подключаю колонку»: шаги подключения и переноса, справа — «Ждать не нужно», готово или что делать. */
function Connecting({
    live,
    only,
    busy,
    onRestore,
    error,
}: {
    live: SpeakerLink;
    only: boolean;
    busy: boolean;
    onRestore: () => void;
    error: string | null;
}) {
    const t = tr.onbConnect;
    const phase = live.phase;
    const c = live.connect?.steps;
    const linked = live.wasConnected || live.arrivedLate || live.connect?.status === 'done';
    const connectTone = (ids: string[]): Tone => {
        if (linked) return 'ok';
        const tone = toneOf(c, ids, false);
        return phase === 'timeout' && tone === 'busy' ? 'warn' : tone;
    };
    const moved = phase === 'done' && !!live.to && !live.moveError;
    const moveTone = (ids: string[], first = false): Tone => {
        if (moved) return 'ok';
        if (live.moveError && !live.move) return first ? 'bad' : 'wait';
        return toneOf(live.move?.steps, ids, live.move?.status === 'done');
    };
    const rows: Row[] = live.to
        ? [
              { title: t.stepBack, sub: t.stepBackSub, tone: connectTone(['copy']) },
              { title: t.stepLink, sub: t.stepLinkSub, tone: connectTone(['write', 'verify', 'reboot', 'arrive']) },
              { title: t.stepInstall, sub: t.stepInstallSub, tone: moveTone(['check', 'copy', 'install'], true) },
              { title: t.stepVoice, sub: t.stepVoiceSub, tone: moveTone(['settings', 'point']) },
              { title: t.stepArrive, sub: only ? t.stepArriveOnly : t.stepArriveHelper, tone: moveTone(['arrive']) },
          ]
        : [
              { title: t.stepBack, sub: t.stepBackSub, tone: connectTone(['copy']) },
              { title: t.stepVoicePc, sub: t.stepVoicePcSub, tone: connectTone(['write', 'verify']) },
              { title: t.stepReboot, sub: t.stepRebootSub, tone: connectTone(['reboot']) },
              { title: t.stepArrive, sub: t.stepArrivePc, tone: connectTone(['arrive']) },
          ];
    // Полоска — у строки «DuoWave — на колонку», пока она идёт (как на доске).
    if (live.to && rows[2].tone === 'busy') rows[2] = { ...rows[2], bar: true };

    const working = phase === 'connecting' || phase === 'moving';
    const title =
        phase === 'done'
            ? moved && only
                ? t.movedTitle
                : t.doneTitle
            : phase === 'restoring' || phase === 'restored'
              ? t.restoreTitle
              : only && live.to
                ? t.movingTitle
                : t.connectingTitle(live.name);
    const url = live.panelUrl ?? panelUrlOf(live.address);

    return (
        <div class="onb-cx onb-cx-conn">
            <section class="onb-cx-col onb-cx-l">
                <h1 class="h2">{title}</h1>
                <div style={{ display: 'flex', alignItems: 'center', gap: '14px', padding: '6px 0 4px' }}>
                    <span class="opt-ico">
                        <IconLaptop size="lg" />
                    </span>
                    <span
                        style={{
                            position: 'relative',
                            flex: 1,
                            height: '2px',
                            background: 'repeating-linear-gradient(90deg, var(--own) 0 6px, transparent 6px 11px)',
                        }}
                    >
                        {(working || phase === 'waiting') && (
                            <span
                                style={{
                                    position: 'absolute',
                                    top: '-4px',
                                    left: 0,
                                    width: '10px',
                                    height: '10px',
                                    borderRadius: '50%',
                                    background: 'var(--accent)',
                                    boxShadow: '0 0 10px rgba(182, 243, 106, .8)',
                                    animation: 'onb-hopline 1.6s ease-in-out infinite',
                                }}
                            />
                        )}
                    </span>
                    <span class="opt-ico" style={{ position: 'relative' }}>
                        <svg class="ico ico-lg" viewBox="0 0 24 24" aria-hidden="true">
                            <rect x="6" y="3" width="12" height="18" rx="6" />
                            <path d="M9.5 15h5" />
                        </svg>
                        {moved && (
                            <span class="divi" style={{ position: 'absolute', right: '-10px', top: '-10px', width: '22px', height: '22px' }}>
                                <svg viewBox="0 0 256 256" aria-hidden="true">
                                    <path class="divi-body" d={DIVI_BODY} />
                                    <path class="divi-eye" d="M98 110V138M158 110V138" />
                                </svg>
                            </span>
                        )}
                    </span>
                </div>
                <div class="card" style={{ padding: '6px 22px' }} aria-live="polite">
                    <Checks rows={rows} wide />
                </div>
            </section>

            <section class="onb-cx-col onb-cx-r">
                {working && (
                    <div class="card" style={{ display: 'flex', flexDirection: 'column', gap: '10px' }}>
                        <span class="h3">{t.waitTitle}</span>
                        <p class="small onb-cx-p">{t.waitText}</p>
                        <p class="small onb-cx-p">{t.waitText2}</p>
                    </div>
                )}
                {(phase === 'timeout' || phase === 'waiting') && <NoArrival busy={busy} onRestore={onRestore} />}
                {phase === 'failed' && (
                    <Problem tone="bad" title={t.failedTitle}>
                        <p role="alert" class="small onb-cx-p">
                            {live.error}
                        </p>
                        <p class="small onb-cx-p">{t.failedNote}</p>
                        {!live.restore && (
                            <div class="onb-cx-btns">
                                <button type="button" class="dw-btn dw-btn-light" disabled={busy} onClick={onRestore}>
                                    {t.restore}
                                </button>
                            </div>
                        )}
                    </Problem>
                )}
                {phase === 'restoring' && (
                    <div class="card card-tight" style={{ display: 'flex', alignItems: 'center', gap: '12px' }} role="status">
                        <Mark tone="busy" />
                        <span class="small">{t.restoring}</span>
                    </div>
                )}
                {phase === 'restored' && (
                    <div class="card card-tight" style={{ display: 'flex', alignItems: 'center', gap: '12px' }} role="status">
                        <Mark tone="ok" />
                        <span class="small">{t.restored}</span>
                    </div>
                )}
                {phase === 'done' && moved && only && (
                    <div class="card" style={{ display: 'flex', gap: '16px', alignItems: 'center', borderColor: 'rgba(101, 163, 13, .5)', animation: 'onb-rise .35s ease-out both' }}>
                        <Qr text={`${url}/`} label={t.qrLabel} />
                        <span style={{ display: 'flex', flexDirection: 'column', gap: '6px' }}>
                            <span class="h3">{t.movedCard}</span>
                            <span class="small">
                                {t.movedText}{' '}
                                <a class="q" href={`${url}/`} target="_blank" rel="noreferrer">
                                    {url}
                                </a>
                                {t.movedAfter}
                            </span>
                        </span>
                    </div>
                )}
                {phase === 'done' && !(moved && only) && (
                    <div
                        class="card"
                        style={{ display: 'flex', flexDirection: 'column', gap: '10px', borderColor: 'rgba(101, 163, 13, .5)', animation: 'onb-rise .35s ease-out both' }}
                        role="status"
                    >
                        <span style={{ display: 'flex', alignItems: 'center', gap: '10px' }}>
                            <Mark tone="ok" />
                            <span class="h3">{t.onlineTitle(live.name)}</span>
                        </span>
                        <p class="small onb-cx-p">{moved ? t.onlineHelper : t.onlinePc}</p>
                        {live.moveError !== undefined && (
                            <p class="small onb-cx-note">
                                <span class="mark mark-warn onb-cx-mark-sm">
                                    <IconWarn />
                                </span>
                                {t.moveFailed(live.moveError)}
                            </p>
                        )}
                    </div>
                )}
                {error && (
                    <p role="alert" class="small onb-cx-p onb-cx-bad">
                        {error}
                    </p>
                )}
            </section>
        </div>
    );
}

/** «Колонка не вернулась за 3 минуты» доски Problems: подождать ещё или вернуть как было. */
export function NoArrival({ busy, onRestore }: { busy: boolean; onRestore: () => void }) {
    const t = tr.onbConnect;
    const live = useSpeakerLink();
    const waiting = live?.phase === 'waiting';
    return (
        <Problem tone="warn" icon={<IconClock />} title={t.noArrivalTitle}>
            <p class="small onb-cx-p">{t.noArrivalText}</p>
            <div class="onb-cx-btns">
                <button type="button" class="dw-btn dw-btn-dark" disabled={waiting || busy} onClick={waitMore}>
                    {waiting ? t.waitingMore : t.waitMore}
                </button>
                <button type="button" class="dw-btn dw-btn-light" disabled={busy} onClick={onRestore}>
                    {t.restore}
                </button>
            </div>
        </Problem>
    );
}
