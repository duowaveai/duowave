import { ComponentChildren } from 'preact';
import { useEffect, useRef, useState } from 'preact/hooks';

import { ConnectJob, ConnectStepId, FoundStation, StationCheck, StationNeighbour, api, can, problemOf } from '../api';
import { IconCheck } from '../icons';
import { tr } from '../i18n';
import { drawingOf } from '../stations';
import { confetti } from '../ui/confetti';
import { DuoWaveMark } from '../ui/duowave';
import { Sheet, SkeletonRows, WinSection } from '../ui/sheet';
import { CharactersTab } from './characters';

/** Кнопка занята: крутится внутри неё. */
const Spin = () => <span class="spinner spinner-sm" aria-hidden="true" />;

/** «Шаг 2 из 4» — меткой в шапке мастера. */
const stepChip = (step: number) => <span class="win-chip">{tr.connect.stepOf(step, 4)}</span>;

/** Строка плана в рамке: что изменится; адреса и пути — метками кода. */
function PlanLine({ children }: { children: ComponentChildren }) {
    return (
        <li class="win-row">
            <span class="win-text c-text min-w-0">{children}</span>
        </li>
    );
}

// «Подключить колонку»: мастер из плана открытия (docs/plans/open-source.md, раздел 6). Шлюз находит колонку,
// с согласия читает её настройки, показывает построчно, что изменит, и только по «Подключить» меняет: копия,
// два адреса, проверка, перезагрузка, «колонка пришла». Не пришла за 3 минуты — «Вернуть как было». Для тех, кто не
// пускает шлюз на колонку, — одна команда. Без root и SSH по вики шлюз колонку не подключает.

export const WIKI_ROOT = 'https://hackalice.me/start/';
export const WIKI_SSH = 'https://hackalice.me/start/ssh/';

const stepLabels = (): Record<ConnectStepId, string> => ({
    copy: tr.connect.stepCopy,
    remove: tr.connect.uninstallRemove,
    write: tr.connect.stepWrite,
    verify: tr.connect.stepVerify,
    reboot: tr.connect.stepReboot,
    arrive: tr.connect.stepArrive,
});

const restoreLabels = (): Partial<Record<ConnectStepId, string>> => ({
    write: tr.connect.restoreWrite,
    verify: tr.connect.restoreVerify,
    reboot: tr.connect.restoreReboot,
});

const uninstallLabels = (): Partial<Record<ConnectStepId, string>> => ({
    remove: tr.connect.uninstallRemove,
    verify: tr.connect.uninstallVerify,
    reboot: tr.connect.uninstallReboot,
});

/** Адрес шлюза, каким его видит колонка: адрес страницы, если он домашний; иначе его вводят. */
export function suggestedGateway(): string {
    const host = location.hostname;
    const home = /^(10\.|192\.168\.|172\.(1[6-9]|2\d|3[01])\.)/.test(host) || host.endsWith('.local') || !host.includes('.');
    return home && host !== 'localhost' ? location.host : '';
}

/** Что пошло не так — словами, с тем, что делать. Берут и первые шаги (screens/onb/speaker.tsx). */
export function problemText(problem: string | undefined): string {
    switch (problem) {
        case 'no-ssh':
            return tr.connect.problemNoSsh;
        case 'no-config':
            return tr.connect.problemNoConfig;
        case 'host-key-changed':
            return tr.connect.problemHostKeyChanged;
        case 'auth-failed':
            return tr.connect.problemAuthFailed;
        case 'unreachable':
            return tr.connect.problemUnreachable;
        case 'station-off':
            return tr.connect.problemStationOff;
        case 'no-route':
            return tr.connect.problemNoRoute;
        case 'ssh-slow':
            return tr.connect.problemSshSlow;
        case 'ssh-closed':
            return tr.connect.problemSshClosed;
        case 'no-connect':
            return tr.connect.problemNoConnect;
        case 'no-arrival':
            return tr.connect.problemNoArrival;
        default:
            return problem ?? tr.connect.problemOther;
    }
}

function where(pointsTo: 'yandex' | 'gateway' | 'other' | undefined): string {
    return pointsTo === 'yandex' ? tr.connect.pointsYandex : pointsTo === 'gateway' ? tr.connect.pointsGateway : tr.connect.pointsOther;
}

function Steps({ job, labels }: { job: ConnectJob; labels: Partial<Record<ConnectStepId, string>> }) {
    return (
        <ol class="connect-steps">
            {job.steps.map((step) => (
                <li key={step.id} class={`connect-step is-${step.state}`}>
                    <span class="connect-mark" aria-hidden="true">
                        {step.state === 'done' ? <IconCheck /> : step.state === 'failed' ? '!' : step.state === 'skipped' ? '–' : ''}
                    </span>
                    <span>
                        {labels[step.id] ?? step.id}
                        <span class="sr-only">
                            {step.state === 'done' ? tr.connect.srDone : step.state === 'running' ? tr.connect.srRunning : step.state === 'failed' ? tr.connect.srFailed : ''}
                        </span>
                    </span>
                </li>
            ))}
        </ol>
    );
}

/**
 * Команда, которую нельзя ни потерять, ни опечатать: выделяется целиком, копируется кнопкой. Ею же лист «Подключить
 * шлюз» даёт текст подключения (screens/helper-pc.tsx) — со своей подписью поля.
 */
export function Command({ text, label = tr.connect.commandLabel }: { text: string; label?: string }) {
    const [copied, setCopied] = useState(false);
    const field = useRef<HTMLTextAreaElement>(null);
    // Другой текст — ещё не скопирован.
    useEffect(() => setCopied(false), [text]);
    const copy = async () => {
        try {
            await navigator.clipboard.writeText(text);
            setCopied(true);
        } catch {
            // Буфер обмена — только на HTTPS: выделим, а скопирует человек.
            field.current?.select();
        }
    };
    return (
        <div class="connect-command">
            <textarea ref={field} class="field" readOnly rows={3} value={text} aria-label={label} onFocus={(e) => (e.target as HTMLTextAreaElement).select()} />
            <button class="btn btn-plain btn-sm self-start" onClick={() => void copy()}>
                {copied ? tr.connect.copied : tr.connect.copy}
            </button>
        </div>
    );
}

// Первый запуск в четыре шага (docs/plans/panel-rebuild.md, этап 5): найти → проверить → кто будет отвечать → «скажите…».
// Шаг 2 — проверка и подключение (consent, plan, manual, progress); шаг 3 — персонаж (who); шаг 4 — готово (ready).
type Step = 'find' | 'consent' | 'plan' | 'manual' | 'progress' | 'who' | 'ready';

/** Порт, на который колонка пойдёт к шлюзу: из адреса «хост:порт», без него — порт самого шлюза (own). */
export function gatewayPort(address: string, own = 80): number {
    const port = /:(\d+)\s*$/.exec(address.trim())?.[1];
    return port ? Number(port) : own;
}

/**
 * Главный экран, пока колонок нет: что нужно, что сделает мастер и как вернуть как было — и кнопка, которая его
 * открывает. Без права управлять колонками — только что их подключает администратор.
 */
export function ConnectFirstCard({ onConnect }: { onConnect: () => void }) {
    const mayManage = can('stations.manage');
    return (
        <section class="card glass flex flex-col gap-12" aria-labelledby="connect-first-h">
            <h2 id="connect-first-h" class="t-title m-0">
                {tr.connect.firstTitle}
            </h2>
            <p class="t-meta m-0 c-text">
                {tr.connect.firstIntro}
            </p>
            <ol class="t-meta m-0 connect-first-steps">
                <li>
                    {tr.connect.firstStep1Before}{' '}
                    <a href={WIKI_ROOT} target="_blank" rel="noreferrer">
                        {tr.connect.wikiHackalice}
                    </a>
                    {tr.connect.firstStep1After}
                </li>
                <li>{tr.connect.firstStep2}</li>
                <li>{tr.connect.firstStep3}</li>
            </ol>
            {mayManage ? (
                <button class="dash-done" onClick={onConnect}>
                    {tr.connect.connectSpeaker}
                </button>
            ) : (
                <p class="t-caption m-0">{tr.connect.adminConnects}</p>
            )}
        </section>
    );
}

/**
 * Мастер подключения: от поиска колонки до «скажите колонке…» (этап 5). onConnected — на главную: имя и вид предложит плашка
 * там же; onOpenBrain — «Мозг»,
 * где добавляют свою модель (без неё отвечает Яндекс).
 */
/** «Пульт на телефоне»: адрес этого компьютера в домашней сети — открыть на телефоне в той же сети. */
function PhoneLine() {
    const [addresses, setAddresses] = useState<string[] | null>(null);
    useEffect(() => {
        let alive = true;
        void api
            .placement()
            .then((view) => alive && setAddresses(view.homeAddresses ?? []))
            .catch(() => alive && setAddresses([]));
        return () => {
            alive = false;
        };
    }, []);
    // Пока адрес идёт — место под строкой, чтобы она не сдвинула окно, когда придёт.
    if (addresses === null) return <SkeletonRows rows={1} />;
    const host = addresses[0];
    if (!host) return null;
    // Шлюз отдаёт адрес уже с портом, если он не 80 (station_connect.rs, home_addresses): свой порт — только адресу без него.
    const withPort = /:\d+$/.test(host) || !location.port ? host : `${host}:${location.port}`;
    const url = `http://${withPort}/panel/`;
    return (
        <WinSection title={tr.connect.phoneTitle} group>
            <div class="win-row is-col">
                <span class="win-row-s">{tr.connect.phoneText}</span>
                <span class="win-code self-start">{url}</span>
            </div>
        </WinSection>
    );
}

export function ConnectSheet({
    onClose,
    onConnected,
    onOpenBrain,
    initial,
    bare = false,
}: {
    onClose: () => void;
    onConnected: (device: string) => void;
    onOpenBrain?: () => void;
    /** Колонка и адрес шлюза заранее: «Переподключить», когда шлюз сменил порт. */
    initial?: { address: string; gateway: string };
    /** Первые шаги (docs/plans/onboarding.md): персонаж и «готово» — их шаги, мастер кончается подключением. */
    bare?: boolean;
}) {
    const [step, setStep] = useState<Step>('find');
    const [found, setFound] = useState<FoundStation[] | null>(null);
    const [address, setAddress] = useState(initial?.address ?? '');
    const [gateway, setGateway] = useState(initial?.gateway ?? suggestedGateway());
    /** Порт самого шлюза: на него колонка пойдёт, если адрес введён без порта. */
    const [ownPort, setOwnPort] = useState(80);
    const [check, setCheck] = useState<StationCheck | null>(null);
    const [job, setJob] = useState<ConnectJob | null>(null);
    /** Что сейчас делается: прочитать колонку, подключить, вернуть как было. Кнопки ждут, крутится — у того, что нажали. */
    const [doing, setDoing] = useState<'read' | 'connect' | 'restore' | null>(null);
    const busy = doing !== null;
    const [error, setError] = useState<string | null>(null);

    // Колонка вышла на связь — салют, один раз за подключение.
    const cheered = useRef<string | null>(null);
    useEffect(() => {
        if (job?.status !== 'done' || job.kind === 'restore' || cheered.current === job.id) return;
        cheered.current = job.id;
        confetti();
    }, [job?.id, job?.status]);

    /** ДиВи ведёт мастер: в шапке каждого шага он живой и оглядывается (ui/divi.ts, 'lively'). */
    const guide = <DuoWaveMark size={40} live idle="lively" />;

    useEffect(() => {
        let live = true;
        api.discoverStations()
            .then((result) => {
                if (!live) return;
                setFound(result.stations);
                if (result.port) setOwnPort(result.port);
                // Пульт открыт с этого компьютера (127.0.0.1): адрес для колонки подскажет сам шлюз.
                if (result.gateway?.length) setGateway((current) => current || result.gateway![0]);
            })
            .catch(() => live && setFound([]));
        return () => {
            live = false;
        };
    }, []);

    // Подключение идёт на шлюзе: пульт смотрит на него, пока оно не кончится.
    useEffect(() => {
        if (!job || job.status !== 'running') return;
        const timer = setInterval(() => {
            api.connectJob(job.id)
                .then(setJob)
                .catch(() => undefined);
        }, 1200);
        return () => clearInterval(timer);
    }, [job?.id, job?.status]);

    const run = async (what: 'read' | 'connect' | 'restore', work: () => Promise<void>) => {
        setDoing(what);
        setError(null);
        try {
            await work();
        } catch (e) {
            setError(problemOf(e));
        } finally {
            setDoing(null);
        }
    };

    const read = () =>
        run('read', async () => {
            const result = await api.checkStation(address.trim(), gateway.trim() || undefined);
            setCheck(result);
            setStep('plan');
        });
    const connect = () =>
        run('connect', async () => {
            setJob(await api.connectStation(check!.address, gateway.trim()));
            setStep('progress');
        });
    const restore = () =>
        run('restore', async () => {
            setJob(await api.restoreStation(check!.address, 'previous'));
        });

    const problem = error && (
        <p role="alert" class="win-callout m-0">
            {error}
        </p>
    );

    if (step === 'find')
        return (
            <Sheet
                title={tr.connect.findTitle}
                about={tr.connect.findHint}
                meta={stepChip(1)}
                icon={guide}
                onClose={onClose}
                footer={
                    <>
                        <button class="btn btn-plain" onClick={onClose}>
                            {tr.connect.cancel}
                        </button>
                        <button class="btn btn-main" disabled={!address.trim()} onClick={() => setStep('consent')}>
                            {tr.connect.next}
                        </button>
                    </>
                }
            >
                {found === null ? (
                    <SkeletonRows rows={2} label={tr.connect.searching} />
                ) : found.length ? (
                    <WinSection title={tr.connect.foundLabel}>
                        <div role="radiogroup" aria-label={tr.connect.foundLabel} class="win-group">
                            {found.map((station) => {
                                const on = address === station.address;
                                return (
                                    <button
                                        key={station.address}
                                        role="radio"
                                        aria-checked={on}
                                        class="win-row win-choice text-left"
                                        onClick={() => setAddress(station.address)}
                                    >
                                        <span class="win-row-main">
                                            <span class="win-row-t">
                                                {station.txt.platform ? drawingOf(null, station.txt.platform).title : station.name || tr.connect.speakerDefault}
                                            </span>
                                            <span class="win-row-s">
                                                <span class="win-code">{station.address}</span> {station.ssh ? tr.connect.sshYes : tr.connect.sshNo}
                                            </span>
                                        </span>
                                        <span class={`dash-radio${on ? ' on' : ''}`} aria-hidden="true">
                                            {on && <IconCheck />}
                                        </span>
                                    </button>
                                );
                            })}
                        </div>
                    </WinSection>
                ) : (
                    <p class="win-callout is-calm m-0">{tr.connect.noneFound}</p>
                )}
                <label class="flex flex-col gap-6">
                    <span class="win-label">{tr.connect.addressLabel}</span>
                    <input
                        class="field"
                        value={address}
                        inputMode="decimal"
                        placeholder={tr.connect.addressPlaceholder}
                        aria-label={tr.connect.addressLabel}
                        onInput={(event) => setAddress((event.target as HTMLInputElement).value)}
                        onKeyDown={(event) => {
                            if (event.key === 'Enter' && address.trim()) setStep('consent');
                        }}
                    />
                </label>
                <p class="win-note">
                    {tr.connect.lanHint}{' '}
                    <a href={WIKI_ROOT} target="_blank" rel="noreferrer">
                        {tr.connect.wikiStep1}
                    </a>
                    {tr.connect.lanHintMid}{' '}
                    <a href={WIKI_SSH} target="_blank" rel="noreferrer">
                        {tr.connect.wikiStep2}
                    </a>
                    .
                </p>
            </Sheet>
        );

    if (step === 'consent')
        return (
            <Sheet
                title={tr.connect.consentTitle}
                meta={
                    <>
                        {stepChip(2)}
                        <span class="win-chip">{address.trim()}</span>
                    </>
                }
                icon={guide}
                onClose={onClose}
                footer={
                    <>
                        <button class="btn btn-plain" disabled={busy} onClick={() => setStep('find')}>
                            {tr.connect.back}
                        </button>
                        <button class="btn btn-main" disabled={busy} aria-busy={busy || undefined} onClick={() => void read()}>
                            {busy && <Spin />}
                            {busy ? tr.connect.reading : tr.connect.read}
                        </button>
                    </>
                }
            >
                <p class="win-lead">{tr.connect.consentText}</p>
                <label class="flex flex-col gap-6">
                    <span class="win-label">{tr.connect.gatewayAddress}</span>
                    <input
                        class="field"
                        value={gateway}
                        placeholder={tr.connect.gatewayPlaceholder}
                        aria-label={tr.connect.gatewayLabel}
                        onInput={(event) => setGateway((event.target as HTMLInputElement).value)}
                    />
                    <span class="win-hint">{tr.connect.portNote(gatewayPort(gateway, ownPort))}</span>
                </label>
                {problem}
            </Sheet>
        );

    if (step === 'plan' && check) {
        const facts = check.facts;
        const ready = !check.problem && !!facts && !!check.target;
        const neighbours = check.neighbours ?? [];
        const blocked = neighbours.some((neighbour) => neighbour.severity === 'blocks');
        const factChips = facts ? [facts.deviceType && drawingOf(null, facts.deviceType).title, facts.softwareVersion && tr.connect.firmware(facts.softwareVersion)] : [];
        return (
            <Sheet
                title={check.problem ? tr.connect.planCannot : check.connected ? tr.connect.planAlready : tr.connect.planTitle}
                icon={guide}
                meta={
                    <>
                        {stepChip(2)}
                        {factChips.filter(Boolean).map((fact) => (
                            <span key={fact} class="win-chip">
                                {fact}
                            </span>
                        ))}
                        <span class="win-chip">{check.address}</span>
                    </>
                }
                onClose={onClose}
                footer={
                    <>
                        <button class="btn btn-plain" disabled={busy} onClick={() => setStep('consent')}>
                            {tr.connect.back}
                        </button>
                        {ready && !check.connected && (
                            <>
                                <button class="btn btn-plain" disabled={busy} onClick={() => setStep('manual')}>
                                    {tr.connect.doItMyself}
                                </button>
                                <button
                                    class="btn btn-main"
                                    disabled={busy || !gateway.trim()}
                                    aria-busy={doing === 'connect' || undefined}
                                    onClick={() => void connect()}
                                >
                                    {doing === 'connect' && <Spin />}
                                    {doing === 'connect' ? tr.connect.connecting : blocked ? tr.connect.connectAnyway : tr.connect.connect}
                                </button>
                            </>
                        )}
                    </>
                }
            >
                {!check.problem && (
                    <Neighbours
                        found={neighbours}
                        address={check.address}
                        busy={busy}
                        checking={doing === 'read'}
                        onCheck={() => void read()}
                        onChanged={() => api.checkStation(check.address, gateway.trim() || undefined).then(setCheck)}
                    />
                )}
                {check.problem ? (
                    <p class="win-callout m-0">
                        <span>
                            {problemText(check.problem)}{' '}
                            {(check.problem === 'no-ssh' || check.problem === 'auth-failed') && (
                                <a href={WIKI_SSH} target="_blank" rel="noreferrer">
                                    {tr.connect.sshWiki}
                                </a>
                            )}
                        </span>
                    </p>
                ) : check.connected ? (
                    <p class="win-lead">{tr.connect.alreadyOnGateway}</p>
                ) : (
                    facts &&
                    check.target && (
                        <>
                            <ul class="win-group m-0 p-0">
                                <PlanLine>
                                    {tr.connect.planVoiceServer} <span class="win-code">{facts.addresses.url}</span> →{' '}
                                    <span class="win-code">{check.target.url}</span>
                                </PlanLine>
                                <PlanLine>
                                    {tr.connect.planQuic} <span class="win-code">{facts.addresses.quic || tr.connect.emptyWord}</span>
                                    {tr.connect.planQuicAfter}
                                </PlanLine>
                                <PlanLine>
                                    {tr.connect.planFwBefore} <span class="win-code">quasar.cfg</span> {tr.connect.planFwMid}{' '}
                                    <span class="win-code">/data/opt/duowave</span>
                                    {tr.connect.planFwAfter} <span class="win-code">/data/opt/duowave/restore.sh</span>.
                                </PlanLine>
                                <PlanLine>{tr.connect.planReboot}</PlanLine>
                            </ul>
                            {facts.pointsTo === 'gateway' ? (
                                <p class="win-callout m-0">
                                    <span>
                                        {tr.connect.otherGatewayBefore} <span class="win-code">{facts.addresses.url}</span>
                                        {tr.connect.otherGatewayAfter}
                                    </span>
                                </p>
                            ) : (
                                <p class="win-callout is-calm m-0">{tr.connect.voiceGoes(where(facts.pointsTo))}</p>
                            )}
                            {check.updates === 'on' && (
                                <p class="win-callout m-0">
                                    <span>
                                        {tr.connect.updatesBefore}{' '}
                                        <a href={WIKI_SSH} target="_blank" rel="noreferrer">
                                            {tr.connect.updatesLink}
                                        </a>
                                        .
                                    </span>
                                </p>
                            )}
                            {check.copy?.refused === 'firmware-changed' && <p class="win-callout m-0">{tr.connect.firmwareChanged}</p>}
                            {check.copy?.refused === 'gateway-off' && <p class="win-callout is-calm m-0">{tr.connect.gatewayOff}</p>}
                            {check.dataFreeKb !== undefined && check.dataFreeKb < 2048 && <p class="win-callout is-calm m-0">{tr.connect.lowSpace}</p>}
                        </>
                    )
                )}
                {problem}
            </Sheet>
        );
    }

    if (step === 'manual' && check) {
        const command = `ssh -p 2222 root@${check.address} 'wget -qO- http://${gateway.trim()}/station/connect.sh | sh'`;
        return (
            <Sheet
                title={tr.connect.manualTitle}
                about={tr.connect.manualAbout}
                icon={guide}
                onClose={onClose}
                footer={
                    <button class="btn btn-plain" onClick={() => setStep('plan')}>
                        {tr.connect.back}
                    </button>
                }
            >
                <Command text={command} />
                <p class="win-text m-0">
                    {tr.connect.manualBefore} <span class="win-code">ssh -p 2222 root@{check.address} sh /data/opt/duowave/restore.sh</span>
                    {tr.connect.manualAfter}
                </p>
            </Sheet>
        );
    }

    // Шаг 3: подключена — первое решение человека, приятное и понятное: кто будет отвечать. Персонаж — это голос и манера.
    if (step === 'ready' && job?.device)
        return (
            <Sheet
                title={tr.connect.whoTitle}
                about={tr.connect.whoHint}
                meta={
                    <>
                        {stepChip(3)}
                        <span class="pill pill-yandex">{tr.connect.pillYandex}</span>
                    </>
                }
                onClose={onClose}
                footer={
                    <button class="btn btn-main" onClick={() => setStep('who')}>
                        {tr.connect.next}
                    </button>
                }
            >
                <p class="win-lead">{tr.connect.connectedNote}</p>
                <CharactersTab />
            </Sheet>
        );

    // Шаг 4: готово — живой голос. Имя и вид колонки — позже, плашкой на главной; пульт на телефоне — по адресу.
    if (step === 'who' && job?.device) {
        const device = job.device;
        return (
            <Sheet
                title={tr.connect.readyTitle}
                meta={stepChip(4)}
                onClose={() => onConnected(device)}
                footer={
                    <button class="btn btn-main" onClick={() => onConnected(device)}>
                        {tr.connect.openHome}
                    </button>
                }
            >
                {/* ДиВи радуется вместе с салютом (эффект выше), потом снова оглядывается. */}
                <div class="connect-hero">
                    <DuoWaveMark size={72} live cheer idle="lively" />
                </div>
                <p class="win-text m-0">{tr.connect.sayToSpeaker}</p>
                <p class="connect-say m-0 self-start">{tr.connect.sayExample}</p>
                <PhoneLine />
            </Sheet>
        );
    }

    // progress: подключение, а при «не пришла» — откат на том же экране. Закрыть, когда кончилось, — крестиком.
    const restoring = job?.kind === 'restore';
    return (
        <Sheet
            title={restoring ? tr.connect.restoringTitle : tr.connect.connectingTitle}
            meta={check?.address ? <span class="win-chip">{check.address}</span> : undefined}
            onClose={onClose}
            dismissable={job?.status !== 'running'}
            footer={
                job?.status === 'done' && !restoring ? (
                    <button class="btn btn-main" onClick={() => (bare && job.device ? onConnected(job.device) : setStep('ready'))}>
                        {tr.connect.next}
                    </button>
                ) : job?.status === 'timeout' ? (
                    <>
                        <button class="btn btn-plain" disabled={busy} onClick={onClose}>
                            {tr.connect.keep}
                        </button>
                        <button class="btn btn-main" disabled={busy} aria-busy={doing === 'restore' || undefined} onClick={() => void restore()}>
                            {doing === 'restore' && <Spin />}
                            {tr.connect.restoreBack}
                        </button>
                    </>
                ) : job?.status === 'running' ? (
                    <button class="btn btn-main" disabled aria-busy="true">
                        <Spin />
                        {tr.connect.wait}
                    </button>
                ) : undefined
            }
        >
            {!restoring && (
                <div class="connect-hero">
                    <DuoWaveMark size={72} live idle="lively" />
                </div>
            )}
            {job && <Steps job={job} labels={restoring ? restoreLabels() : stepLabels()} />}
            {job?.status === 'timeout' && (
                <p class="win-callout m-0">
                    {problemText('no-arrival')} {tr.connect.timeoutHint(gatewayPort(gateway, ownPort))}
                </p>
            )}
            {job?.status === 'failed' && (
                <p role="alert" class="win-callout m-0">
                    {problemText(job.error)}
                    {job.copy ? tr.connect.ourCopy(job.copy) : ''}
                </p>
            )}
            {job?.status === 'done' && restoring && <p class="win-lead">{tr.connect.restoredOk}</p>}
            {problem}
        </Sheet>
    );
}

/** «Вернуть к Яндексу»: прежние адреса голосового сервера и перезагрузка — по подтверждению. DuoWave остаётся. */
export function RestoreSheet(props: { address: string; name: string; onClose: () => void }) {
    return <WayBackSheet {...props} mode="restore" />;
}

/** «Удалить DuoWave с колонки»: uninstall.sh по SSH, проверка, что ничего нашего не осталось, перезагрузка. */
export function UninstallSheet(props: { address: string; name: string; onClose: () => void }) {
    return <WayBackSheet {...props} mode="uninstall" />;
}

function WayBackSheet({
    address,
    name,
    onClose,
    mode,
}: {
    address: string;
    name: string;
    onClose: () => void;
    mode: 'restore' | 'uninstall';
}) {
    const removing = mode === 'uninstall';
    const [job, setJob] = useState<ConnectJob | null>(null);
    const [busy, setBusy] = useState(false);
    const [error, setError] = useState<string | null>(null);

    useEffect(() => {
        if (!job || job.status !== 'running') return;
        const timer = setInterval(() => {
            api.connectJob(job.id)
                .then(setJob)
                .catch(() => undefined);
        }, 1200);
        return () => clearInterval(timer);
    }, [job?.id, job?.status]);

    const start = async () => {
        setBusy(true);
        setError(null);
        try {
            setJob(await (removing ? api.uninstallStation(address) : api.restoreStation(address)));
        } catch (e) {
            setError(problemOf(e));
        } finally {
            setBusy(false);
        }
    };

    return (
        <Sheet
            title={removing ? tr.connect.uninstallTitle : tr.connect.restoreTitle}
            meta={
                <>
                    <span class="win-chip">{name}</span>
                    <span class="win-chip">{address}</span>
                </>
            }
            onClose={onClose}
            dismissable={job?.status !== 'running'}
            footer={
                !job ? (
                    <>
                        <button class="btn btn-plain" disabled={busy} onClick={onClose}>
                            {tr.connect.cancel}
                        </button>
                        <button class={`btn ${removing ? 'btn-danger-fill' : 'btn-main'}`} disabled={busy} aria-busy={busy || undefined} onClick={() => void start()}>
                            {busy && <Spin />}
                            {busy ? tr.connect.starting : removing ? tr.connect.uninstall : tr.connect.restore}
                        </button>
                    </>
                ) : job.status === 'running' ? (
                    <button class="btn btn-main" disabled aria-busy="true">
                        <Spin />
                        {tr.connect.wait}
                    </button>
                ) : undefined
            }
        >
            {!job ? (
                <p class="win-lead">{removing ? tr.connect.uninstallText : tr.connect.restoreText}</p>
            ) : (
                <Steps job={job} labels={removing ? uninstallLabels() : restoreLabels()} />
            )}
            {job?.status === 'failed' && (
                <p role="alert" class="win-callout m-0">
                    <span>
                        {problemText(job.error)} {tr.connect.withoutGateway}{' '}
                        <span class="win-code">
                            ssh -p 2222 root@{address} sh /data/opt/duowave/{removing ? 'uninstall.sh' : 'restore.sh'}
                        </span>
                    </span>
                </p>
            )}
            {job?.status === 'done' && <p class="win-lead">{removing ? tr.connect.uninstallDone : tr.connect.restoreDone}</p>}
            {error && (
                <p role="alert" class="win-callout m-0">
                    {error}
                </p>
            )}
        </Sheet>
    );
}

/** Слова о чужом на колонке: имя, чем мешает, как выключить его же способом и как включить обратно. */
interface NeighbourWords {
    name: string;
    why: string;
    off: string;
    cmd: string;
    back: string;
}

/**
 * Что на колонке может помешать DuoWave — до кнопки «Подключить», как и все предупреждения. Каждое — строкой-гармошкой:
 * метка и имя видны сразу, чем мешает и как выключить — по нажатию. Важное (с ним DuoWave не заработает) подсвечено,
 * чтобы его раскрыли. У мода со своим выключателем — «Выключить»: шлюз выполнит команду самого мода (ничего не удаляя)
 * после «Да», а пульт посмотрит колонку снова. Остальное человек выключает сам.
 */
function Neighbours({
    found,
    address,
    busy,
    checking,
    onCheck,
    onChanged,
}: {
    found: StationNeighbour[];
    address: string;
    /** Мастер занят (читает колонку, подключает): «Проверить снова» ждёт. */
    busy: boolean;
    /** Колонку читают заново по «Проверить снова». */
    checking: boolean;
    onCheck: () => void;
    onChanged: () => Promise<void>;
}) {
    const [asking, setAsking] = useState<string | null>(null);
    const [working, setWorking] = useState<string | null>(null);
    const [failed, setFailed] = useState<{ id: string; text: string } | null>(null);
    if (!found.length) return null;
    const known = tr.connect.neighbour as Record<string, NeighbourWords | undefined>;
    const blocking = found.some((neighbour) => neighbour.severity === 'blocks');
    const turnOff = async (id: string) => {
        setWorking(id);
        setFailed(null);
        try {
            await api.turnOffStationMod(address, id);
            setAsking(null);
            await onChanged();
        } catch (e) {
            setFailed({ id, text: problemOf(e) });
        } finally {
            setWorking(null);
        }
    };
    return (
        <WinSection title={tr.connect.neighboursTitle} aside={found.length}>
            {blocking && <p class="win-callout m-0">{tr.connect.neighboursOpen}</p>}
            {found.map((neighbour) => {
                const words = known[neighbour.id];
                const memory = neighbour.id === 'low-memory' ? neighbour.detail : undefined;
                const blocks = neighbour.severity === 'blocks';
                // В строке — только имя файла: полный путь внутри.
                const short = neighbour.detail && !memory ? neighbour.detail.split('/').pop() : undefined;
                const rowClass = ['connect-neighbour', blocks && 'connect-neighbour-blocks', neighbour.off && 'connect-neighbour-off'].filter(Boolean).join(' ');
                return (
                    <details key={`${neighbour.id} ${neighbour.detail ?? ''}`} class={rowClass}>
                        <summary>
                            <span class="connect-neighbour-tag">
                                {neighbour.off ? tr.connect.neighboursOff : blocks ? tr.connect.neighboursBlocks : tr.connect.neighboursWarns}
                            </span>
                            <b>{words?.name ?? neighbour.id}</b>
                            {short && <span class="win-code">{short}</span>}
                            {memory && <span class="t-caption">{tr.connect.megabytes(memory)}</span>}
                        </summary>
                        <div class="connect-neighbour-body t-meta">
                            {neighbour.detail && !memory && <span class="win-code self-start">{neighbour.detail}</span>}
                            {neighbour.off ? (
                                <p class="m-0">{tr.connect.offNote}</p>
                            ) : (
                                <p class="m-0">{memory ? tr.connect.freeMemory(memory) : words?.why}</p>
                            )}
                            {!neighbour.off && words?.off && (
                                <p class="m-0">
                                    {words.off}
                                    {words.cmd && (
                                        <>
                                            {' '}
                                            <span class="win-code">{words.cmd}</span>
                                        </>
                                    )}
                                </p>
                            )}
                            {neighbour.switch && asking !== neighbour.id && (
                                <button class="btn btn-plain btn-sm self-start" disabled={working !== null} onClick={() => setAsking(neighbour.id)}>
                                    {tr.connect.turnOff}
                                </button>
                            )}
                            {neighbour.switch && asking === neighbour.id && (
                                <div class="connect-neighbour-ask">
                                    <p class="m-0">{tr.connect.turnOffAsk}</p>
                                    <span class="win-code self-start">{neighbour.switch}</span>
                                    {words?.back && (
                                        <p class="m-0">
                                            {tr.connect.turnBack} <span class="win-code">{words.back}</span>
                                        </p>
                                    )}
                                    <div class="flex gap-8">
                                        <button
                                            class="btn btn-plain btn-sm"
                                            disabled={working !== null}
                                            aria-busy={working === neighbour.id || undefined}
                                            onClick={() => void turnOff(neighbour.id)}
                                        >
                                            {working === neighbour.id && <Spin />}
                                            {working === neighbour.id ? tr.connect.turningOff : tr.connect.turnOffYes}
                                        </button>
                                        <button class="btn btn-quiet btn-sm" disabled={working !== null} onClick={() => setAsking(null)}>
                                            {tr.connect.cancel}
                                        </button>
                                    </div>
                                </div>
                            )}
                            {neighbour.off && words?.back && (
                                <p class="m-0">
                                    {tr.connect.turnBack} <span class="win-code">{words.back}</span>
                                </p>
                            )}
                            {failed?.id === neighbour.id && (
                                <p role="alert" class="win-callout m-0">
                                    {failed.text}
                                </p>
                            )}
                        </div>
                    </details>
                );
            })}
            <p class="win-note">{tr.connect.neighboursNote}</p>
            <button class="btn btn-plain btn-sm self-start" disabled={busy} aria-busy={checking || undefined} onClick={onCheck}>
                {checking && <Spin />}
                {checking ? tr.connect.reading : tr.connect.checkAgain}
            </button>
        </WinSection>
    );
}
