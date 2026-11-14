import { useEffect, useRef, useState } from 'preact/hooks';

import { ConnectJob, ConnectStepId, FoundStation, StationCheck, api, can, problemOf } from '../api';
import { IconCheck } from '../icons';
import { tr } from '../i18n';
import { drawingOf } from '../stations';
import { confetti } from '../ui/confetti';
import { DuoWaveMark } from '../ui/duowave';
import { Sheet } from '../ui/sheet';
import { CharactersTab } from './characters';

// «Подключить колонку»: мастер из плана открытия (docs/plans/open-source.md, раздел 6). Шлюз находит колонку,
// с согласия читает её настройки, показывает построчно, что изменит, и только по «Подключить» меняет: копия,
// два адреса, проверка, перезагрузка, «колонка пришла». Не пришла за 3 минуты — «Вернуть как было». Для тех, кто не
// пускает шлюз на колонку, — одна команда. Без root и SSH по вики шлюз колонку не подключает.

const WIKI_ROOT = 'https://hackalice.me/start/';
const WIKI_SSH = 'https://hackalice.me/start/ssh/';

const stepLabels = (): Record<ConnectStepId, string> => ({
    copy: tr.connect.stepCopy,
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

/** Адрес шлюза, каким его видит колонка: адрес страницы, если он домашний; иначе его вводят. */
export function suggestedGateway(): string {
    const host = location.hostname;
    const home = /^(10\.|192\.168\.|172\.(1[6-9]|2\d|3[01])\.)/.test(host) || host.endsWith('.local') || !host.includes('.');
    return home && host !== 'localhost' ? location.host : '';
}

/** Что пошло не так — словами, с тем, что делать. */
function problemText(problem: string | undefined): string {
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

/** Команда, которую нельзя ни потерять, ни опечатать: выделяется целиком, копируется кнопкой. */
function Command({ text }: { text: string }) {
    const [copied, setCopied] = useState(false);
    const field = useRef<HTMLTextAreaElement>(null);
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
            <textarea ref={field} class="field" readOnly rows={3} value={text} aria-label={tr.connect.commandLabel} onFocus={(e) => (e.target as HTMLTextAreaElement).select()} />
            <button class="pill pill-off" onClick={() => void copy()}>
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
    const host = addresses?.[0];
    if (!host) return null;
    // Шлюз отдаёт адрес уже с портом, если он не 80 (station_connect.rs, home_addresses): свой порт — только адресу без него.
    const withPort = /:\d+$/.test(host) || !location.port ? host : `${host}:${location.port}`;
    const url = `http://${withPort}/panel/`;
    return (
        <div class="connect-phone">
            <b>{tr.connect.phoneTitle}</b>
            <span class="t-meta">{tr.connect.phoneText}</span>
            <span class="connect-path">{url}</span>
        </div>
    );
}

export function ConnectSheet({
    onClose,
    onConnected,
    onOpenBrain,
    initial,
}: {
    onClose: () => void;
    onConnected: (device: string) => void;
    onOpenBrain?: () => void;
    /** Колонка и адрес шлюза заранее: «Переподключить», когда шлюз сменил порт. */
    initial?: { address: string; gateway: string };
}) {
    const [step, setStep] = useState<Step>('find');
    const [found, setFound] = useState<FoundStation[] | null>(null);
    const [address, setAddress] = useState(initial?.address ?? '');
    const [gateway, setGateway] = useState(initial?.gateway ?? suggestedGateway());
    /** Порт самого шлюза: на него колонка пойдёт, если адрес введён без порта. */
    const [ownPort, setOwnPort] = useState(80);
    const [check, setCheck] = useState<StationCheck | null>(null);
    const [job, setJob] = useState<ConnectJob | null>(null);
    const [busy, setBusy] = useState(false);
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

    const run = async (work: () => Promise<void>) => {
        setBusy(true);
        setError(null);
        try {
            await work();
        } catch (e) {
            setError(problemOf(e));
        } finally {
            setBusy(false);
        }
    };

    const read = () =>
        run(async () => {
            const result = await api.checkStation(address.trim(), gateway.trim() || undefined);
            setCheck(result);
            setStep('plan');
        });
    const connect = () =>
        run(async () => {
            setJob(await api.connectStation(check!.address, gateway.trim()));
            setStep('progress');
        });
    const restore = () =>
        run(async () => {
            setJob(await api.restoreStation(check!.address, 'previous'));
        });

    const problem = error && (
        <p role="alert" class="t-meta m-0 c-dot-offline">
            {error}
        </p>
    );

    if (step === 'find')
        return (
            <Sheet
                title={tr.connect.findTitle}
                about={tr.connect.findAbout}
                icon={guide}
                onClose={onClose}
                footer={
                    <div class="dash-actions">
                        <button class="dash-done" disabled={!address.trim()} onClick={() => setStep('consent')}>
                            {tr.connect.next}
                        </button>
                        <button class="pill pill-off dash-wide" onClick={onClose}>
                            {tr.connect.cancel}
                        </button>
                    </div>
                }
            >
                {found === null ? (
                    <p class="t-meta m-0">
                        {tr.connect.searching}
                    </p>
                ) : found.length ? (
                    <div role="radiogroup" aria-label={tr.connect.foundLabel} class="flex flex-col">
                        {found.map((station) => (
                            <button
                                key={station.address}
                                role="radio"
                                aria-checked={address === station.address}
                                class="picker-row"
                                onClick={() => setAddress(station.address)}
                            >
                                <span class="dash-option-text">
                                    <b>{station.txt.platform ? drawingOf(null, station.txt.platform).title : station.name || tr.connect.speakerDefault}</b>
                                    <span class="t-caption">
                                        {station.address} · {station.ssh ? tr.connect.sshYes : tr.connect.sshNo}
                                    </span>
                                </span>
                                <span class={`dash-radio${address === station.address ? ' on' : ''}`} aria-hidden="true">
                                    {address === station.address && <IconCheck />}
                                </span>
                            </button>
                        ))}
                    </div>
                ) : (
                    <p class="t-meta m-0">
                        {tr.connect.noneFound}
                    </p>
                )}
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
                <p class="t-caption m-0">
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
                about={tr.connect.step2About(address.trim())}
                icon={guide}
                onClose={onClose}
                footer={
                    <div class="dash-actions">
                        <button class="dash-done" disabled={busy} onClick={() => void read()}>
                            {busy ? tr.connect.reading : tr.connect.read}
                        </button>
                        <button class="pill pill-off dash-wide" disabled={busy} onClick={() => setStep('find')}>
                            {tr.connect.back}
                        </button>
                    </div>
                }
            >
                <p class="t-meta m-0 c-text">
                    {tr.connect.consentText}
                </p>
                <label class="t-caption flex flex-col gap-6">
                    {tr.connect.gatewayAddress}
                    <input
                        class="field"
                        value={gateway}
                        placeholder={tr.connect.gatewayPlaceholder}
                        aria-label={tr.connect.gatewayLabel}
                        onInput={(event) => setGateway((event.target as HTMLInputElement).value)}
                    />
                </label>
                <p class="t-caption m-0">
                    {tr.connect.portNote(gatewayPort(gateway, ownPort))}
                </p>
                {problem}
            </Sheet>
        );

    if (step === 'plan' && check) {
        const facts = check.facts;
        const ready = !check.problem && !!facts && !!check.target;
        return (
            <Sheet
                title={check.problem ? tr.connect.planCannot : check.connected ? tr.connect.planAlready : tr.connect.planTitle}
                icon={guide}
                about={tr.connect.step2About(
                    facts
                        ? [facts.deviceType && drawingOf(null, facts.deviceType).title, facts.softwareVersion && tr.connect.firmware(facts.softwareVersion), check.address]
                              .filter(Boolean)
                              .join(' · ')
                        : check.address,
                )}
                onClose={onClose}
                footer={
                    <div class="dash-actions">
                        {ready && !check.connected && (
                            <button class="dash-done" disabled={busy || !gateway.trim()} onClick={() => void connect()}>
                                {busy ? tr.connect.connecting : tr.connect.connect}
                            </button>
                        )}
                        {ready && !check.connected && (
                            <button class="pill pill-off dash-wide" disabled={busy} onClick={() => setStep('manual')}>
                                {tr.connect.doItMyself}
                            </button>
                        )}
                        {(!ready || check.connected) && (
                            <button class="dash-done" onClick={onClose}>
                                {tr.connect.close}
                            </button>
                        )}
                        <button class="pill pill-off dash-wide" disabled={busy} onClick={() => setStep('consent')}>
                            {tr.connect.back}
                        </button>
                    </div>
                }
            >
                {check.problem ? (
                    <p class="t-meta m-0 c-text">
                        {problemText(check.problem)}{' '}
                        {(check.problem === 'no-ssh' || check.problem === 'auth-failed') && (
                            <a href={WIKI_SSH} target="_blank" rel="noreferrer">
                                {tr.connect.sshWiki}
                            </a>
                        )}
                    </p>
                ) : check.connected ? (
                    <p class="t-meta m-0 c-text">
                        {tr.connect.alreadyOnGateway}
                    </p>
                ) : (
                    facts &&
                    check.target && (
                        <>
                            <ul class="connect-plan">
                                <li>
                                    {tr.connect.planVoiceServer} <code>{facts.addresses.url}</code> → <code>{check.target.url}</code>
                                </li>
                                <li>
                                    {tr.connect.planQuic} <code>{facts.addresses.quic || tr.connect.emptyWord}</code>
                                    {tr.connect.planQuicAfter}
                                </li>
                                <li>
                                    {tr.connect.planFwBefore} <code>quasar.cfg</code> {tr.connect.planFwMid} <code>/data/opt/duowave</code>
                                    {tr.connect.planFwAfter} <code>/data/opt/duowave/restore.sh</code>.
                                </li>
                                <li>{tr.connect.planReboot}</li>
                            </ul>
                            {facts.pointsTo === 'gateway' ? (
                                <p class="t-meta connect-warning m-0">
                                    {tr.connect.otherGatewayBefore} <code>{facts.addresses.url}</code>
                                    {tr.connect.otherGatewayAfter}
                                </p>
                            ) : (
                                <p class="t-caption m-0">
                                    {tr.connect.voiceGoes(where(facts.pointsTo))}
                                </p>
                            )}
                            {check.updates === 'on' && (
                                <p class="t-meta connect-warning m-0">
                                    {tr.connect.updatesBefore}{' '}
                                    <a href={WIKI_SSH} target="_blank" rel="noreferrer">
                                        {tr.connect.updatesLink}
                                    </a>
                                    .
                                </p>
                            )}
                            {check.copy?.refused === 'firmware-changed' && (
                                <p class="t-meta connect-warning m-0">
                                    {tr.connect.firmwareChanged}
                                </p>
                            )}
                            {check.copy?.refused === 'gateway-off' && (
                                <p class="t-meta m-0">
                                    {tr.connect.gatewayOff}
                                </p>
                            )}
                            {check.dataFreeKb !== undefined && check.dataFreeKb < 2048 && (
                                <p class="t-meta m-0">
                                    {tr.connect.lowSpace}
                                </p>
                            )}
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
                    <div class="dash-actions">
                        <button class="dash-done" onClick={onClose}>
                            {tr.connect.done}
                        </button>
                        <button class="pill pill-off dash-wide" onClick={() => setStep('plan')}>
                            {tr.connect.back}
                        </button>
                    </div>
                }
            >
                <Command text={command} />
                <p class="t-caption m-0">
                    {tr.connect.manualBefore} <code>ssh -p 2222 root@{check.address} sh /data/opt/duowave/restore.sh</code>
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
                about={tr.connect.whoAbout}
                onClose={onClose}
                footer={
                    <div class="dash-actions">
                        <button class="dash-done" onClick={() => setStep('who')}>
                            {tr.connect.next}
                        </button>
                    </div>
                }
            >
                <p class="connect-now m-0">
                    <span class="pill pill-yandex">{tr.connect.pillYandex}</span>
                    <span class="t-meta">{tr.connect.connectedNote}</span>
                </p>
                <CharactersTab />
            </Sheet>
        );

    // Шаг 4: готово — живой голос. Имя и вид колонки — позже, плашкой на главной; пульт на телефоне — по адресу.
    if (step === 'who' && job?.device) {
        const device = job.device;
        return (
            <Sheet
                title={tr.connect.readyTitle}
                about={tr.connect.step4About}
                onClose={() => onConnected(device)}
                footer={
                    <div class="dash-actions">
                        <button class="dash-done" onClick={() => onConnected(device)}>
                            {tr.connect.openHome}
                        </button>
                    </div>
                }
            >
                {/* ДиВи радуется вместе с салютом (эффект выше), потом снова оглядывается. */}
                <div class="connect-hero">
                    <DuoWaveMark size={72} live cheer idle="lively" />
                </div>
                <p class="t-meta m-0">{tr.connect.sayToSpeaker}</p>
                <p class="connect-say m-0">{tr.connect.sayExample}</p>
                <PhoneLine />
            </Sheet>
        );
    }

    // progress: подключение, а при «не пришла» — откат на том же экране.
    const restoring = job?.kind === 'restore';
    return (
        <Sheet
            title={restoring ? tr.connect.restoringTitle : tr.connect.connectingTitle}
            about={check?.address}
            onClose={onClose}
            dismissable={job?.status !== 'running'}
            footer={
                job?.status === 'done' && !restoring ? (
                    <button class="dash-done" onClick={() => setStep('ready')}>
                        {tr.connect.next}
                    </button>
                ) : job?.status === 'timeout' ? (
                    <div class="dash-actions">
                        <button class="dash-done" disabled={busy} onClick={() => void restore()}>
                            {tr.connect.restoreBack}
                        </button>
                        <button class="pill pill-off dash-wide" disabled={busy} onClick={onClose}>
                            {tr.connect.keep}
                        </button>
                    </div>
                ) : job?.status === 'running' ? (
                    <button class="dash-done" disabled>
                        {tr.connect.wait}
                    </button>
                ) : (
                    <button class="dash-done" onClick={onClose}>
                        {tr.connect.close}
                    </button>
                )
            }
        >
            {!restoring && (
                <div class="connect-hero">
                    <DuoWaveMark size={72} live idle="lively" />
                </div>
            )}
            {job && <Steps job={job} labels={restoring ? restoreLabels() : stepLabels()} />}
            {job?.status === 'timeout' && (
                <p class="t-meta m-0 c-text">
                    {problemText('no-arrival')} {tr.connect.timeoutHint(gatewayPort(gateway, ownPort))}
                </p>
            )}
            {job?.status === 'failed' && (
                <p role="alert" class="t-meta m-0 c-text">
                    {problemText(job.error)}
                    {job.copy ? tr.connect.ourCopy(job.copy) : ''}
                </p>
            )}
            {job?.status === 'done' && restoring && (
                <p class="t-meta m-0 c-text">
                    {tr.connect.restoredOk}
                </p>
            )}
            {problem}
        </Sheet>
    );
}

/** «Вернуть к Яндексу»: прежние адреса голосового сервера и перезагрузка — по подтверждению. */
export function RestoreSheet({ address, name, onClose }: { address: string; name: string; onClose: () => void }) {
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
            setJob(await api.restoreStation(address));
        } catch (e) {
            setError(problemOf(e));
        } finally {
            setBusy(false);
        }
    };

    return (
        <Sheet
            title={tr.connect.restoreTitle}
            about={`${name} · ${address}`}
            onClose={onClose}
            dismissable={job?.status !== 'running'}
            footer={
                !job ? (
                    <div class="dash-actions">
                        <button class="dash-done" disabled={busy} onClick={() => void start()}>
                            {busy ? tr.connect.starting : tr.connect.restore}
                        </button>
                        <button class="pill pill-off dash-wide" disabled={busy} onClick={onClose}>
                            {tr.connect.cancel}
                        </button>
                    </div>
                ) : (
                    <button class="dash-done" disabled={job.status === 'running'} onClick={onClose}>
                        {job.status === 'running' ? tr.connect.wait : tr.connect.close}
                    </button>
                )
            }
        >
            {!job ? (
                <p class="t-meta m-0 c-text">
                    {tr.connect.restoreText}
                </p>
            ) : (
                <Steps job={job} labels={restoreLabels()} />
            )}
            {job?.status === 'failed' && (
                <p role="alert" class="t-meta m-0 c-text">
                    {problemText(job.error)} {tr.connect.withoutGateway} <code>ssh -p 2222 root@{address} sh /data/opt/duowave/restore.sh</code>
                </p>
            )}
            {job?.status === 'done' && (
                <p class="t-meta m-0 c-text">
                    {tr.connect.restoreDone}
                </p>
            )}
            {error && (
                <p role="alert" class="t-meta m-0 c-dot-offline">
                    {error}
                </p>
            )}
        </Sheet>
    );
}
