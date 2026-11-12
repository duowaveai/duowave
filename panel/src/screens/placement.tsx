import { useEffect, useRef, useState } from 'preact/hooks';

import { HomeServerStatus, MoveJob, MoveTo, PlacementView, Station, StationGatewayView, api, can, problemOf } from '../api';
import { FeatureRow, GROUP_TITLES, PLACEMENT_TITLES, Placement, useAvailability } from '../availability';
import { timeAgo } from '../time';
import { Host, Solved, solve } from '../truth';
import { DashData, ourBrain, scenarioOf } from './dash-model';
import { Gear, NodeIcon, PlacementMap, WhoThree } from './placement-map';
import { Action } from '../ui/cards';
import { SecretField, TextField } from '../ui/fields';
import { Sheet } from '../ui/sheet';
import { Switch } from '../ui/switch';
import { ask } from '../ui/confirm';

/**
 * «Где работает шлюз» (docs/plans/station-placement.md): на компьютере, на колонке или на колонке с компьютером как
 * домашним сервером. Пульт компьютера показывает шлюз на колонке, каким его видит компьютер, и переносит шлюз; пульт
 * колонки — себя, свой домашний сервер и путь назад к Яндексу без компьютера. Что где доступно — список шлюза.
 */

const STEP_TITLES: Record<MoveJob['steps'][number]['id'], string> = {
    check: 'Проверка колонки: процессор, место, память',
    copy: 'Путь назад: копия адресов и restore.sh',
    install: 'Шлюз на колонку: сборка этого компьютера',
    settings: 'Настройки',
    point: 'Голос колонки — к шлюзу, перезагрузка',
    arrive: 'Колонка на связи',
    stop: 'Шлюз на колонке остановлен',
};

const MOVE_TITLES: Record<MoveTo, string> = {
    station: 'Перенос шлюза на колонку',
    stationServer: 'Перенос шлюза на колонку с домашним сервером',
    pc: 'Возврат шлюза на компьютер',
    yandex: 'Колонка — снова Яндексу',
};

const LEVEL_MARK: Record<FeatureRow['pc']['level'], string> = { yes: '✓', partial: '◐', no: '—' };

function mb(kb?: number): string {
    return kb === undefined ? '—' : `${Math.round(kb / 1024)} МБ`;
}

/** «держит 10 МБ памяти, свободно 61 МБ · в /data свободно 40 МБ» — только то, что известно. */
function resources(memory?: { rssKb?: number; availableKb?: number }, data?: { freeKb: number }): string {
    const parts = [];
    if (memory?.rssKb !== undefined) parts.push(`держит ${mb(memory.rssKb)} памяти`);
    if (memory?.availableKb !== undefined) parts.push(`свободно ${mb(memory.availableKb)}`);
    if (data) parts.push(`в /data свободно ${mb(data.freeKb)}`);
    return parts.join(', ');
}

function clock(at?: string): string {
    if (!at) return '';
    const date = new Date(at);
    const time = date.toLocaleTimeString('ru-RU', { hour: '2-digit', minute: '2-digit' });
    return date.toDateString() === new Date().toDateString() ? time : `${date.toLocaleDateString('ru-RU')} ${time}`;
}

function Line({ children, tone }: { children: preact.ComponentChildren; tone?: 'warn' | 'error' | 'ok' }) {
    const color = tone === 'warn' ? ' c-pill-yandex-fg' : tone === 'error' ? ' c-dot-offline' : tone === 'ok' ? ' c-accent-text' : '';
    return (
        <p class={`t-caption m-0${color}`} role={tone === 'error' ? 'alert' : tone ? 'status' : undefined}>
            {children}
        </p>
    );
}

function Row({ children }: { children: preact.ComponentChildren }) {
    return <div class="flex gap-8 flex-wrap items-center">{children}</div>;
}

/** Первая карточка «Дом» → «Шлюз». */
export function PlacementCard() {
    const [view, setView] = useState<PlacementView | null>(null);
    const [message, setMessage] = useState<{ ok: boolean; text: string } | null>(null);
    const [busy, setBusy] = useState<string | null>(null);
    const [choosing, setChoosing] = useState(false);
    const [copies, setCopies] = useState(false);
    const [inspected, setInspected] = useState<Inspected | null>(null);
    const [job, setJob] = useState<MoveJob | null>(null);
    const [linking, setLinking] = useState(false);
    const [unlinking, setUnlinking] = useState(false);
    const [givingBack, setGivingBack] = useState(false);
    const alive = useRef(true);

    const read = () =>
        api
            .placement()
            .then((answer) => {
                if (!alive.current) return;
                setView(answer);
                if (answer.job) setJob(answer.job);
            })
            .catch(() => undefined);

    useEffect(() => {
        alive.current = true;
        void read();
        const timer = window.setInterval(() => void read(), 15_000);
        return () => {
            alive.current = false;
            window.clearInterval(timer);
        };
    }, []);

    // Пока идёт переход — его шаги чаще.
    useEffect(() => {
        if (!job || job.status !== 'running') return;
        const timer = window.setInterval(() => {
            api.moveJob(job.id)
                .then((next) => {
                    setJob(next);
                    if (next.status !== 'running') void read();
                })
                .catch(() => undefined);
        }, 1500);
        return () => window.clearInterval(timer);
    }, [job?.id, job?.status]);

    if (!view) return null;

    const act = async (name: string, work: () => Promise<unknown>, done?: string) => {
        setBusy(name);
        setMessage(null);
        try {
            await work();
            if (done) setMessage({ ok: true, text: done });
            await read();
        } catch (e) {
            setMessage({ ok: false, text: problemOf(e) });
        } finally {
            setBusy(null);
        }
    };

    const mayManage = can('stations.manage');
    // Листы — рядом с карточкой, не внутри: у стекла backdrop-filter, и fixed внутри него держится карточки.
    return (
        <>
            <section class="card glass px-16 pt-4 pb-14 flex flex-col gap-10" aria-labelledby="placement-h">
                <div class="section-head">
                    <h2 id="placement-h" class="t-title-xs m-0">
                        Где работает DuoWave
                    </h2>
                    <span class="pill pill-own">{PLACEMENT_TITLES[view.role === 'pc' ? (view.station?.record?.placement ?? 'pc') : view.placement]}</span>
                </div>
                {view.role === 'station' ? (
                    <StationSide view={view} busy={busy} act={act} mayManage={mayManage} onGiveBack={() => setGivingBack(true)} />
                ) : (
                    <ComputerSide
                        view={view}
                        busy={busy}
                        act={act}
                        mayManage={mayManage}
                        onChoose={() => setChoosing(true)}
                        onCopies={() => setCopies(true)}
                        onInspect={() =>
                            void act('inspect', async () => {
                                setInspected(await api.inspectStation());
                            })
                        }
                        onLink={() => setLinking(true)}
                        onUnlink={() => setUnlinking(true)}
                    />
                )}
                {job && <MoveProgress job={job} />}
                {message && <Line tone={message.ok ? 'ok' : 'error'}>{message.text}</Line>}
            </section>
            {linking && (
                <LinkSheet
                    onClose={() => setLinking(false)}
                    onLinked={() => {
                        setLinking(false);
                        void read();
                    }}
                />
            )}
            {choosing && (
                <PlacementChooser
                    view={view}
                    onClose={() => setChoosing(false)}
                    onStarted={(started) => {
                        setJob(started);
                        setChoosing(false);
                    }}
                />
            )}
            {copies && view.station && (
                <CopiesSheet
                    station={view.station}
                    onClose={() => setCopies(false)}
                    onRestore={(id) =>
                        act('restore', () => api.restoreStationSettings(id), 'Копия отправлена: шлюз на колонке перезапустится на ней.')
                    }
                    onBackup={() => act('backup', () => api.backupStation(), 'Копия настроек колонки сохранена на этом компьютере.')}
                />
            )}
            {inspected && <InspectSheet inspected={inspected} onClose={() => setInspected(null)} />}
            {givingBack && (
                <GiveBackSheet
                    onClose={() => setGivingBack(false)}
                    onConfirm={async () => {
                        await act('giveBack', () => api.giveBack(), 'Колонка перезагружается: через минуту отвечает Алиса, DuoWave ждёт здесь.');
                        setGivingBack(false);
                    }}
                />
            )}
            {unlinking && view.station?.record && (
                <UnlinkSheet
                    onClose={() => setUnlinking(false)}
                    onConfirm={async () => {
                        const address = view.station!.record!.address;
                        await act('yandex', async () => {
                            setJob(await api.startMove({ to: 'yandex', address }));
                        });
                        setUnlinking(false);
                    }}
                />
            )}
        </>
    );
}

type Act = (name: string, work: () => Promise<unknown>, done?: string) => Promise<void>;

/** «Что случилось»: колонка о своём шлюзе — по её оболочке, даже когда сломана сама программа. */
type Inspected = Awaited<ReturnType<typeof api.inspectStation>>;

/** Пульт компьютера: шлюз на колонке, каким его видит компьютер. */
function ComputerSide({
    view,
    busy,
    act,
    mayManage,
    onChoose,
    onCopies,
    onInspect,
    onLink,
    onUnlink,
}: {
    view: PlacementView;
    busy: string | null;
    act: Act;
    mayManage: boolean;
    onChoose: () => void;
    onCopies: () => void;
    onInspect: () => void;
    onLink: () => void;
    onUnlink: () => void;
}) {
    const station = view.station;
    const record = station?.record;
    const onStation = record && record.placement !== 'pc';
    if (!onStation) {
        return (
            <>
                <Line>
                    Шлюз работает на этом компьютере: колонка шлёт голос сюда. Пока компьютер спит, колонка молчит — шлюз можно
                    перенести на саму колонку.
                </Line>
                {station?.payload.source === 'none' && <Line tone="warn">{station.payload.note}</Line>}
                {mayManage && (
                    <Row>
                        <Action label="Сменить…" onClick={onChoose} />
                        <Action label="Шлюз уже на колонке…" onClick={onLink} />
                    </Row>
                )}
            </>
        );
    }
    const live = station.live;
    const health = live.health?.placement;
    const memory = health?.memory;
    const update = station.update;
    const homeServer = health?.homeServer;
    return (
        <>
            <Line>
                {live.reachable
                    ? [`Шлюз на колонке ${station.version ?? ''} на связи`, resources(memory, health?.data)].filter(Boolean).join(' · ')
                    : live.seenAt
                      ? `Шлюз на колонке не отвечает · последний раз ${timeAgo(live.seenAt)}`
                      : 'Шлюз на колонке ещё не ответил.'}
            </Line>
            <label class="helper-row">
                <span class="helper-icon">
                    <SegIcon placement="pc" />
                </span>
                <span class="helper-text">
                    Компьютер помогает колонке
                    <span>
                        {record.placement !== 'stationServer'
                            ? 'выключено: колонка обходится без компьютера'
                            : homeServer?.state === 'down'
                              ? 'колонка не дотягивается до компьютера: работает через облако или Яндекс'
                              : 'слух, свой голос и модели — у этого компьютера'}
                    </span>
                </span>
                <Switch
                    on={record.placement === 'stationServer'}
                    label="Компьютер помогает колонке"
                    disabled={!mayManage || busy === 'helper' || !live.reachable}
                    onChange={(on) =>
                        void act('helper', () => api.setStationHelper(on), on ? 'Колонка опирается на этот компьютер.' : 'Колонка работает без компьютера.')
                    }
                />
            </label>
            {!station.compatible && <Line tone="warn">Шлюз на колонке говорит на другой версии API: сначала обновите его.</Line>}
            {station.warnings.map((warning) => (
                <div key={warning.kind} class="flex flex-col gap-6">
                    <Line tone="warn">{warning.text}</Line>
                    {mayManage && warning.kind === 'silent' && (
                        <Row>
                            <Action label="Проверить" busy={busy === 'inspect'} onClick={onInspect} />
                            <Action
                                label="Попробовать снова"
                                busy={busy === 'retry'}
                                onClick={() => void act('retry', () => api.retryStation(), 'Шлюз на колонке запущен, голос возвращается к нему.')}
                            />
                        </Row>
                    )}
                    {mayManage && warning.kind === 'voiceAway' && (
                        <Row>
                            <Action
                                label="Вернуть голос DuoWave"
                                busy={busy === 'voice'}
                                onClick={() =>
                                    void act('voice', () => api.stationVoice('local'), 'Колонка перезагружается: через минуту её голос — у шлюза.')
                                }
                            />
                        </Row>
                    )}
                </div>
            ))}
            <Row>
                <a class="pill pill-off py-8 px-14" href={`http://${record.address}:${record.port}/panel`} target="_blank" rel="noreferrer">
                    Пульт колонки ↗
                </a>
                {mayManage && <Action label="Сменить…" onClick={onChoose} />}
            </Row>
            <SettingsLine view={view} busy={busy} act={act} mayManage={mayManage} onCopies={onCopies} />
            <div class="flex flex-col gap-6">
                {update ? (
                    <Row>
                        <Line>Есть шлюз для колонки новее: {update}.</Line>
                        {can('system.update') && (
                            <Action
                                tone="on"
                                label={live.updating ? 'обновляется…' : `Обновить колонку до ${update}`}
                                busy={live.updating || busy === 'update'}
                                onClick={() => void act('update', () => api.updateStationGateway())}
                            />
                        )}
                    </Row>
                ) : (
                    live.reachable && <Line>У шлюза на колонке последняя версия.</Line>
                )}
                {live.updateError && <Line tone="error">Колонка не обновилась: {live.updateError}</Line>}
                {can('system.update') && record.managed && (
                    <label class="t-caption flex items-center gap-10">
                        <Switch
                            on={record.autoUpdate}
                            label="Обновлять колонку автоматически"
                            disabled={busy === 'auto'}
                            onChange={(on) => void act('auto', () => api.setStationAutoUpdate(on))}
                        />
                        Обновлять колонку вместе с компьютером, не спрашивая
                    </label>
                )}
                {mayManage && health && (
                    <label class="t-caption flex items-center gap-10">
                        <Switch
                            on={health.safety !== false}
                            label="Страховка"
                            disabled={busy === 'safety' || !live.reachable}
                            onChange={(on) => void act('safety', () => api.setSafety(on))}
                        />
                        Страховка: шлюз не поднимается — колонка вернётся к прежнему адресу
                    </label>
                )}
            </div>
            {mayManage && (
                <button class="danger-link" disabled={busy === 'yandex'} onClick={onUnlink}>
                    Отвязать колонку и вернуть Яндексу…
                </button>
            )}
        </>
    );
}

/**
 * Настройки колонки и их копии (docs/plans/desk.md, этап 4): настройки живут на колонке, помощник правит их там, в её
 * окнах, а здесь держит копии. Выбора «на колонке / на компьютере» больше нет; колонка, чьи настройки пока на
 * компьютере (так было прежде), возвращает их на колонку одной кнопкой.
 */
function SettingsLine({
    view,
    busy,
    act,
    mayManage,
    onCopies,
}: {
    view: PlacementView;
    busy: string | null;
    act: Act;
    mayManage: boolean;
    onCopies: () => void;
}) {
    const station = view.station!;
    const record = station.record!;
    const latest = station.backups[0];
    const live = station.live;
    if (record.settingsHome === 'pc')
        return (
            <div class="flex flex-col gap-6">
                <Line tone="warn">
                    Настройки этой колонки пока хранятся на компьютере и уходят на неё отсюда. Теперь они живут на колонке:
                    верните их туда — помощник будет править их прямо на колонке и держать копии здесь.
                </Line>
                {live.syncError && <Line tone="error">{live.syncError}</Line>}
                {mayManage && (
                    <Row>
                        <Action
                            label="Вернуть настройки на колонку"
                            busy={busy === 'home'}
                            onClick={() =>
                                void ask({
                                    title: 'Настройки — на колонку?',
                                    text: 'Последние настройки с компьютера уйдут на колонку, и дальше она хранит их сама; компьютер держит копии.',
                                    ok: 'Вернуть',
                                }).then((yes) => yes && void act('home', () => api.setSettingsHome('station')))
                            }
                        />
                    </Row>
                )}
            </div>
        );
    const text = station.copyStale
        ? latest
            ? `Настройки — на колонке · компьютер не видел колонку с ${clock(latest.at)} — копия старая`
            : 'Настройки — на колонке · копии на этом компьютере ещё нет'
        : `Настройки — на колонке, правятся в её окнах · копия здесь — ${latest ? timeAgo(latest.at) : '—'}`;
    return (
        <div class="flex flex-col gap-6">
            <Line tone={station.copyStale ? 'warn' : undefined}>{text}</Line>
            {live.syncError && <Line tone="error">{live.syncError}</Line>}
            {mayManage && (
                <Row>
                    <Action label="Копии…" onClick={onCopies} />
                </Row>
            )}
        </div>
    );
}

/** Пульт колонки: шлюз на ней, её домашний сервер, путь назад к Яндексу без компьютера. */
function StationSide({
    view,
    busy,
    act,
    mayManage,
    onGiveBack,
}: {
    view: PlacementView;
    busy: string | null;
    act: Act;
    mayManage: boolean;
    onGiveBack: () => void;
}) {
    const health = view.health;
    const voiceHere = health.voice?.local !== false;
    const managed =
        view.managedBy === 'pc'
            ? 'Обновления ставит компьютер, который поставил шлюз.'
            : view.managedBy === 'yam'
              ? 'Шлюз поставлен через YAM: обновление — yam install файла новой версии из выпуска, на колонке.'
              : view.managedBy === 'yse'
                ? 'Шлюз поставлен через yse: его обновляет yse.'
                : 'Обновления — из ленты выпусков, в карточке «Обновления».';
    return (
        <>
            <Line>{['Шлюз работает на самой колонке', resources(health.memory, health.data)].filter(Boolean).join(' · ')}</Line>
            <StationHelper mayManage={can('connectors.manage')} onChanged={() => void act('helperRead', async () => undefined)} />
            <Line>
                {view.state.settingsHome === 'pc'
                    ? `Настройки этой колонки пока приходят с компьютера — верните их на колонку в его пульте: «Настройки» → «Колонка».${
                          health.settingsReceivedAt ? ` Получены ${timeAgo(health.settingsReceivedAt)}.` : ''
                      }`
                    : health.settingsCopiedAt
                      ? `Настройки хранятся здесь · копия на компьютере — ${timeAgo(health.settingsCopiedAt)}`
                      : 'Настройки хранятся здесь · компьютер ещё не брал их копию'}
            </Line>
            <Line>{managed}</Line>
            {view.state.safetyLast && (
                <Line tone="warn">
                    Страховка сработала {clock(view.state.safetyLast.at)}:{' '}
                    {view.state.safetyLast.what === 'previous-version' ? 'вернулась прежняя версия.' : 'колонка возвращалась к прежнему адресу.'}
                </Line>
            )}
            {mayManage && (
                <label class="t-caption flex items-center gap-10">
                    <Switch
                        on={view.state.safety}
                        label="Страховка"
                        disabled={busy === 'safety'}
                        onChange={(on) => void act('safety', () => api.setSafety(on))}
                    />
                    Страховка: шлюз не поднимается — колонка вернётся к прежнему адресу
                </label>
            )}
            {mayManage && (
                <Row>
                    {voiceHere ? (
                        <button class="danger-link soft" disabled={busy === 'giveBack'} onClick={onGiveBack}>
                            Отдать голос Яндексу…
                        </button>
                    ) : (
                        <>
                            <Line tone="warn">Голос колонки сейчас не у этого шлюза.</Line>
                            <Action
                                label="Вернуть голос DuoWave"
                                busy={busy === 'pointLocal'}
                                onClick={() =>
                                    void act('pointLocal', () => api.pointLocal(), 'Колонка перезагружается: через минуту её голос — у шлюза.')
                                }
                            />
                        </>
                    )}
                </Row>
            )}
        </>
    );
}

/** Шаги перехода. */
function MoveProgress({ job }: { job: MoveJob }) {
    return (
        <div role="status" class="flex flex-col gap-4 py-10 px-12 r-panel bg-divider">
            <span class="fw-700 fs-13">
                {MOVE_TITLES[job.to]}
                {job.status === 'done' ? ' — готово' : job.status === 'failed' ? ' — не вышло' : '…'}
            </span>
            {job.steps.map((step) => (
                <span key={step.id} class={`t-caption flex gap-8${step.state === 'pending' || step.state === 'skipped' ? ' opacity-50' : ''}`}>
                    <span aria-hidden="true" class="w-14">
                        {step.state === 'done' ? '✓' : step.state === 'failed' ? '✕' : step.state === 'running' ? '…' : step.state === 'skipped' ? '–' : '·'}
                    </span>
                    {STEP_TITLES[step.id]}
                </span>
            ))}
            {job.note && <Line>{job.note}</Line>}
            {job.error && <Line tone="error">{job.error}</Line>}
        </div>
    );
}

/** Таблица «Что доступно» для одного размещения: что есть, что частично, чего нет — и почему. */
function FeatureList({ rows, placement }: { rows: FeatureRow[]; placement: Placement }) {
    const groups = [...new Set(rows.map((row) => row.group))];
    return (
        <div class="flex flex-col gap-8">
            {groups.map((group) => (
                <div key={group} class="flex flex-col gap-2">
                    <span class="t-meta fw-700">
                        {GROUP_TITLES[group] ?? group}
                    </span>
                    {rows
                        .filter((row) => row.group === group)
                        .map((row) => {
                            const cell = row[placement];
                            return (
                                <span key={row.id} class={`t-caption flex gap-8${cell.level === 'no' ? ' opacity-55' : ''}`}>
                                    <span aria-label={cell.level === 'yes' ? 'есть' : cell.level === 'partial' ? 'частично' : 'нет'} class="w-14 flex-none">
                                        {LEVEL_MARK[cell.level]}
                                    </span>
                                    <span class="flex-1">
                                        {row.title}
                                        {cell.note !== 'да' && cell.note !== 'нет' && <span class="t-meta"> — {cell.note}</span>}
                                    </span>
                                </span>
                            );
                        })}
                </div>
            ))}
        </div>
    );
}

/** Что будет при каждом размещении — коротко, по четыре строки: переключение сегментов не двигает страницу. */
const WHAT: Record<Placement, Array<['plus' | 'minus' | 'info', string]>> = {
    pc: [
        ['plus', 'Работает всё, включая «Своё» и звонки.'],
        ['minus', 'Компьютер уснул — колонка молчит.'],
        ['info', 'На колонку ничего не ставится.'],
        ['info', 'Перенос на колонку — тем же окном.'],
    ],
    station: [
        ['plus', 'Компьютер не нужен: колонка работает сама.'],
        ['minus', 'Без «Своего» и своего голоса.'],
        ['minus', 'Без облачного слуха отвечает Яндекс.'],
        ['info', 'Компьютер можно позвать на помощь позже.'],
    ],
    stationServer: [
        ['plus', 'Пока компьютер на связи — всё как дома.'],
        ['plus', 'Компьютер уснул — колонка сама переходит в «Вместе».'],
        ['minus', 'Без компьютера и облака слух у Яндекса.'],
        ['info', 'Помощь компьютера — просто переключатель.'],
    ],
};

const SEG_TITLES: Record<Placement, string> = { pc: 'На компьютере', station: 'На колонке', stationServer: 'Колонка + компьютер' };

function SegIcon({ placement }: { placement: Placement }) {
    return (
        <svg width="22" height="22" viewBox="0 0 24 24" aria-hidden="true">
            {placement === 'stationServer' ? (
                <>
                    <g transform="translate(-3.5 3) scale(0.75)">
                        <NodeIcon host="station" />
                    </g>
                    <g transform="translate(9.5 3) scale(0.65)">
                        <NodeIcon host="pc" />
                    </g>
                </>
            ) : (
                <NodeIcon host={placement === 'pc' ? 'pc' : 'station'} />
            )}
        </svg>
    );
}

/** «Сменить…»: где работает DuoWave — три варианта, схема, что будет, связь без адресов (решения 2, 6, 7). */
function PlacementChooser({ view, onClose, onStarted }: { view: PlacementView; onClose: () => void; onStarted: (job: MoveJob) => void }) {
    const availability = useAvailability();
    const record = view.station?.record;
    const current: Placement = record?.placement ?? 'pc';
    const [picked, setPicked] = useState<Placement>(current === 'pc' ? 'stationServer' : current);
    const [stations, setStations] = useState<Station[] | null>(null);
    const [address, setAddress] = useState(record?.address ?? '');
    const [gateway, setGateway] = useState('');
    const [editing, setEditing] = useState<{ station: boolean; pc: boolean }>({ station: false, pc: false });
    // Настройки колонки живут на ней (docs/plans/desk.md, этап 4): перенос на колонку их туда и кладёт.
    const home = 'station' as const;
    const [error, setError] = useState<string | null>(null);
    const [busy, setBusy] = useState(false);
    const [data, setData] = useState<DashData>({ mode: 'gateway', availability });

    useEffect(() => {
        void Promise.all([
            api.mode().catch(() => null),
            api.health().catch(() => null),
            can('connectors.view') ? api.connectors().catch(() => null) : null,
            can('station.view') ? api.models().catch(() => null) : null,
            can('connectors.view') ? api.brain().catch(() => null) : null,
        ]).then(([mode, health, connectors, models, brain]) =>
            setData({
                mode: mode?.mode ?? health?.panelMode ?? 'gateway',
                routes: health?.skills?.routes,
                brain: brain?.status ?? health?.brain,
                brainView: brain,
                models,
                connectors: connectors?.connectors ?? null,
                availability,
            }),
        );
        void api
            .stations()
            .then((list) => {
                setStations(list);
                if (!address) {
                    const known = list.find((entry) => entry.status === 'approved' && entry.lastAddress);
                    if (known?.lastAddress) setAddress(known.lastAddress);
                }
            })
            .catch(() => setStations([]));
    }, []);

    const to: MoveTo = picked;
    const same = picked === current;
    const onStation = picked !== 'pc';
    // Сборки для колонки здесь нет и взять её неоткуда: перенести на колонку нечего.
    const noBuild = onStation && view.station?.payload.source === 'none';
    const full = { ...data, availability };
    // Окно объясняет, как может работать, а не только как сейчас: облако в схеме считается подключённым даже без
    // ключей. Настоящее состояние — рядом припиской, когда облачного распознавания или голоса на самом деле нет.
    const real = scenarioOf(full, picked);
    const solved = solve({ ...real, cloudEar: true, cloudVoice: true });
    const brain = real.brain;
    const cloudAssumed = !real.cloudEar || !real.cloudVoice;
    // Имя выбранной модели — на карточку «Модели и агенты»; никакой не выбрано — список примеров.
    const chosen = ourBrain(data.brain);
    const brainName = chosen.usable && chosen.tone === 'cloud' ? chosen.name : undefined;
    const station = stations?.find((entry) => entry.lastAddress === address.trim());
    const pcAddress = gateway.trim() || bestHome(view.homeAddresses ?? [], address.trim());
    const settingsAt: Host = onStation ? 'station' : 'pc';
    // Шлюз уже на колонке: «На колонке» ↔ «Колонка + ПК» — переключатель, без переноса.
    const helperOnly = current !== 'pc' && onStation && picked !== current;

    const start = async () => {
        setBusy(true);
        setError(null);
        try {
            if (helperOnly) {
                await api.setStationHelper(picked === 'stationServer');
                onClose();
                return;
            }
            const job = await api.startMove({
                to,
                address: address.trim(),
                gateway: gateway.trim() || undefined,
                settingsHome: onStation ? home : undefined,
            });
            onStarted(job);
        } catch (e) {
            setError(problemOf(e));
        } finally {
            setBusy(false);
        }
    };
    const label = busy
        ? 'Начинаю…'
        : same
          ? 'Шлюз уже здесь'
          : helperOnly
            ? picked === 'stationServer'
                ? 'Включить помощь компьютера'
                : 'Выключить помощь компьютера'
            : picked === 'pc'
              ? 'Вернуть шлюз на компьютер'
              : picked === 'station'
                ? 'Перенести на колонку'
                : 'Перенести на колонку с компьютером';
    return (
        <Sheet
            title="Где работает DuoWave"
            about={helperOnly ? 'Без переноса и перезагрузки: колонка просто начнёт или перестанет опираться на компьютер.' : 'Перенос — копия конфига колонки, его правка и перезагрузка: минута без голоса.'}
            onClose={onClose}
            footer={
                <button class="dash-done" disabled={busy || same || noBuild || !address.trim()} onClick={() => void start()}>
                    {label}
                </button>
            }
        >
            <div role="radiogroup" aria-label="Где работает DuoWave" class="place-seg">
                {(['pc', 'station', 'stationServer'] as const).map((id) => (
                    <button key={id} role="radio" aria-checked={picked === id} onClick={() => setPicked(id)}>
                        <SegIcon placement={id} />
                        {SEG_TITLES[id]}
                        <span class="now">{id === current ? 'сейчас' : ''}</span>
                    </button>
                ))}
            </div>

            <div class="settings-mini" role="note" aria-label="Настройки колонки">
                <span class="set-label">
                    <Gear size={16} />
                    Настройки
                </span>
                <p class="meta">
                    {!onStation
                        ? 'Шлюз на компьютере — настройки тоже на нём.'
                        : 'Живут на колонке и меняются и без компьютера; компьютер правит их в её окнах и держит копии.'}
                </p>
            </div>

            <PlacementMap solved={solved} placement={picked} settingsAt={settingsAt} brainName={brainName} />
            {cloudAssumed && (
                <Line>
                    Облако на схеме — как при подключённых ключах: заработает после облачного слуха или голоса в «Приложения» →
                    «Подключения».
                </Line>
            )}

            {onStation ? (
                <label class="helper-row">
                    <span class="helper-icon">
                        <SegIcon placement="pc" />
                    </span>
                    <span class="helper-text">
                        Компьютер помогает колонке
                        <span>
                            {picked === 'stationServer' ? 'слух, свой голос и модели — у компьютера, пока он на связи' : 'выключено: колонка обходится без компьютера'}
                        </span>
                    </span>
                    <Switch on={picked === 'stationServer'} label="Компьютер помогает колонке" onChange={(on) => setPicked(on ? 'stationServer' : 'station')} />
                </label>
            ) : (
                // То же место, тем же размером: в «На компьютере» помощь ПК не нужна — шлюз и есть ПК.
                <div class="helper-row" aria-hidden="true">
                    <span class="helper-icon">
                        <SegIcon placement="pc" />
                    </span>
                    <span class="helper-text">
                        Компьютер помогает колонке
                        <span>не нужно: DuoWave и так на компьютере — колонка говорит с ним напрямую</span>
                    </span>
                </div>
            )}

            <WhoThree ear={solved.ear} brain={solved.brain} voice={solved.voice} />
            {solved.notes.length > 0 && <p class="truth-note">{solved.notes.join(' ')}</p>}

            <div class="flex flex-col gap-8">
                <span class="t-caption fw-700">Что будет</span>
                <ul class="what-list">
                    {WHAT[picked].map(([kind, text]) => (
                        <li key={text}>
                            <span class={`mk ${kind}`} aria-hidden="true">
                                {kind === 'plus' ? '+' : kind === 'minus' ? '−' : '·'}
                            </span>
                            <span>{text}</span>
                        </li>
                    ))}
                </ul>
            </div>

            {!helperOnly && (
                <div class="flex flex-col gap-8">
                    <span class="t-caption fw-700">Связь</span>
                    <div class="link-rows">
                        <div class="link-row">
                            <span class={`link-dot${address.trim() ? '' : ' warn'}`} aria-hidden="true" />
                            <span class="link-text">
                                {station ? `Колонка: ${station.name}` : address.trim() ? 'Колонка' : 'Колонка не найдена'}
                                <span>{address.trim() ? (station ? 'нашлась в «Колонках»' : address.trim()) : 'подключите её в «Колонках» или впишите адрес'}</span>
                            </span>
                            <button class="link-edit" onClick={() => setEditing({ ...editing, station: !editing.station })}>
                                {editing.station ? 'Готово' : 'Изменить'}
                            </button>
                        </div>
                        {(editing.station || !address.trim()) && (
                            <TextField
                                label="Адрес колонки"
                                value={address}
                                placeholder="192.0.2.23"
                                inputMode="url"
                                hint={stations && !stations.length ? 'Колонок пока нет: сначала подключите её в «Колонках».' : undefined}
                                onInput={setAddress}
                            />
                        )}
                        <div class="link-row">
                                <span class={`link-dot${pcAddress ? '' : ' warn'}`} aria-hidden="true" />
                                <span class="link-text">
                                    Компьютер: этот
                                    <span>{pcAddress ? 'виден колонке в домашней сети' : 'адрес в домашней сети не найден — впишите'}</span>
                                </span>
                                <button class="link-edit" onClick={() => setEditing({ ...editing, pc: !editing.pc })}>
                                    {editing.pc ? 'Готово' : 'Изменить'}
                                </button>
                            </div>
                        {(editing.pc || !pcAddress) && (
                            <TextField
                                label="Адрес этого компьютера для колонки"
                                value={gateway}
                                placeholder={pcAddress || '192.0.2.10:3000'}
                                inputMode="url"
                                hint={view.homeAddresses?.length ? `Этот компьютер в домашней сети: ${view.homeAddresses.join(', ')}` : undefined}
                                onInput={setGateway}
                            />
                        )}
                        <div class="link-row">
                            <span class="link-dot" aria-hidden="true" />
                            <span class="link-text">
                                Ключ домашнего сервера
                                <span>{onStation ? 'создастся сам и уедет на колонку при переносе' : 'остаётся на этом компьютере'}</span>
                            </span>
                        </div>
                    </div>
                </div>
            )}

            {!helperOnly && (
                <Line tone={onStation ? 'warn' : undefined}>
                    {onStation
                        ? 'Шлюзу на колонке нужны ключи облачных подключений: они будут лежать на колонке, а её SSH открыт без пароля.'
                        : 'Ключи облачных подключений лежат на этом компьютере — при переносе на колонку уедут вместе со шлюзом.'}
                </Line>
            )}
            {availability && (
                <details>
                    <summary class="t-caption cursor-pointer fw-700">Что доступно — подробно</summary>
                    <div class="pt-8 flex flex-col gap-8">
                        <FeatureList rows={availability.table} placement={picked} />
                        {view.station?.payload && onStation && <Line>{view.station.payload.note}</Line>}
                    </div>
                </details>
            )}
            {error && <Line tone="error">{error}</Line>}
        </Sheet>
    );
}

/** Адрес ПК в сети колонки — как выберет шлюз (watch.rs, pick_home): те же первые три числа, иначе первый. */
function bestHome(addresses: string[], station: string): string {
    const net = (address: string) => address.split(':')[0].split('.').slice(0, 3).join('.');
    return addresses.find((address) => station && net(address) === net(station)) ?? addresses[0] ?? '';
}

/**
 * «Вернуть колонку Яндексу» с ПК: шлюз уходит с колонки совсем, вернуть — только новым переносом. Своё окно: что
 * будет, как вернуться, галочка и отсчёт 5 секунд на красной кнопке (решение 13).
 */
function UnlinkSheet({ onClose, onConfirm }: { onClose: () => void; onConfirm: () => Promise<void> }) {
    const [left, setLeft] = useState(5);
    const [sure, setSure] = useState(false);
    const [busy, setBusy] = useState(false);
    useEffect(() => {
        const timer = window.setInterval(() => setLeft((value) => (value > 0 ? value - 1 : 0)), 1000);
        return () => window.clearInterval(timer);
    }, []);
    return (
        <Sheet
            title="Вернуть колонку Яндексу?"
            onClose={onClose}
            footer={
                <div class="flex flex-col gap-8">
                    <button
                        class="dash-done danger"
                        disabled={left > 0 || !sure || busy}
                        onClick={() => {
                            setBusy(true);
                            void onConfirm().finally(() => setBusy(false));
                        }}
                    >
                        {busy ? 'Колонка перезагружается…' : `Вернуть колонку Яндексу${left > 0 ? ` · ${left}` : ''}`}
                    </button>
                    <button class="dash-done ghost" onClick={onClose}>
                        Отмена
                    </button>
                </div>
            }
        >
            <ul class="what-list">
                <li>
                    <span class="mk minus">−</span>
                    <span>DuoWave уйдёт с колонки. Она станет обычной Алисой, как до переноса.</span>
                </li>
                <li>
                    <span class="mk minus">−</span>
                    <span>Колонка перезагрузится: около минуты без голоса.</span>
                </li>
                <li>
                    <span class="mk info">·</span>
                    <span>Настройки и история останутся на колонке и на компьютере.</span>
                </li>
                <li>
                    <span class="mk plus">+</span>
                    <span>Вернуть DuoWave можно потом: «Сменить…» → перенос на колонку.</span>
                </li>
            </ul>
            <label class="understand">
                <input type="checkbox" checked={sure} onChange={(event) => setSure((event.target as HTMLInputElement).checked)} />
                <span>Понимаю, что колонка станет обычной Алисой, пока я снова не перенесу на неё DuoWave</span>
            </label>
        </Sheet>
    );
}

/** «Отдать голос Яндексу» на колонке: шлюз остаётся, вернуть — той же карточкой. Обычное подтверждение (решение 14). */
function GiveBackSheet({ onClose, onConfirm }: { onClose: () => void; onConfirm: () => Promise<void> }) {
    const [busy, setBusy] = useState(false);
    return (
        <Sheet
            title="Отдать голос Яндексу?"
            onClose={onClose}
            footer={
                <div class="flex flex-col gap-8">
                    <button
                        class="dash-done soft-yandex"
                        disabled={busy}
                        onClick={() => {
                            setBusy(true);
                            void onConfirm().finally(() => setBusy(false));
                        }}
                    >
                        {busy ? 'Колонка перезагружается…' : 'Отдать голос Яндексу'}
                    </button>
                    <button class="dash-done ghost" onClick={onClose}>
                        Отмена
                    </button>
                </div>
            }
        >
            <ul class="what-list">
                <li>
                    <span class="mk minus">−</span>
                    <span>Голос колонки пойдёт Яндексу, отвечать будет Алиса.</span>
                </li>
                <li>
                    <span class="mk info">·</span>
                    <span>DuoWave остаётся на колонке. Колонка перезагрузится, около минуты без голоса.</span>
                </li>
                <li>
                    <span class="mk plus">+</span>
                    <span>Вернуть можно здесь же: «Вернуть голос DuoWave».</span>
                </li>
            </ul>
        </Sheet>
    );
}

/**
 * «ПК помогает колонке» в пульте колонки: один переключатель — адрес и ключ привёз перенос (решение 7). Адрес —
 * только под «Изменить», когда ПК не спит, а связи нет: обычно ПК сам сообщает свой новый адрес (решение 9).
 */
function StationHelper({ mayManage, onChanged }: { mayManage: boolean; onChanged: () => void }) {
    const [status, setStatus] = useState<HomeServerStatus | null>(null);
    const [editing, setEditing] = useState(false);
    const [url, setUrl] = useState('');
    const [key, setKey] = useState('');
    const [busy, setBusy] = useState(false);
    const [message, setMessage] = useState<{ ok: boolean; text: string } | null>(null);
    useEffect(() => {
        void api
            .homeServer()
            .then((answer) => {
                setStatus(answer);
                setUrl(answer.url ?? '');
            })
            .catch(() => setStatus(null));
    }, []);
    if (!status) return null;
    const enabled = status.enabled ?? status.configured;
    const run = async (body: { url?: string; key?: string; enabled?: boolean }) => {
        setBusy(true);
        setMessage(null);
        try {
            const next = await api.linkHomeServer(body);
            setStatus(next);
            setUrl(next.url ?? '');
            setKey('');
            if (body.url || body.key) setEditing(false);
            const on = next.enabled ?? next.configured;
            setMessage(
                !on
                    ? { ok: true, text: 'Колонка работает без компьютера.' }
                    : next.state === 'up'
                      ? { ok: true, text: 'Компьютер на связи.' }
                      : { ok: false, text: next.error ?? 'Компьютер не отвечает.' },
            );
            onChanged();
        } catch (e) {
            setMessage({ ok: false, text: problemOf(e) });
        } finally {
            setBusy(false);
        }
    };
    const sub = !status.configured
        ? 'колонка ещё не знает компьютер — включите в пульте компьютера'
        : !enabled
          ? 'выключено · адрес и ключ уже есть, включится сразу'
          : status.state === 'up'
            ? 'на связи: слух, свой голос и модели — у компьютера'
            : `Компьютер не отвечает${status.since ? ` с ${clock(status.since)}` : ''}`;
    const away = enabled && status.state !== 'up';
    return (
        <div class="flex flex-col gap-8">
            <label class="helper-row">
                <span class="helper-icon">
                    <SegIcon placement="pc" />
                </span>
                <span class="helper-text">
                    Компьютер помогает колонке
                    <span>{sub}</span>
                </span>
                <Switch
                    on={enabled}
                    label="Компьютер помогает колонке"
                    disabled={!mayManage || busy || !status.configured}
                    onChange={(on) => void run({ enabled: on })}
                />
            </label>
            {away && (
                <p class="truth-note">
                    Если компьютер спит — колонка работает сама и подключится, когда он проснётся. Если не спит, а связи нет, его адрес мог
                    смениться: обычно компьютер сообщает новый сам.
                </p>
            )}
            {mayManage && status.configured && (away || editing) && (
                <Row>
                    {status.url && (
                        <a class="pill pill-off py-8 px-14" href={`${status.url}/panel`} target="_blank" rel="noreferrer">
                            Пульт компьютера ↗
                        </a>
                    )}
                    <Action label={editing ? 'Скрыть' : 'Изменить адрес…'} onClick={() => setEditing(!editing)} />
                </Row>
            )}
            {mayManage && editing && (
                <div class="flex flex-col gap-8">
                    <TextField label="Адрес компьютера" value={url} placeholder="http://192.0.2.10:3000" inputMode="url" onInput={setUrl} />
                    <SecretField
                        label="Ключ с компьютера"
                        mask={status.key}
                        value={key}
                        removing={false}
                        hint="Привезён переносом. Заменить — только если компьютер его отозвал."
                        onInput={setKey}
                        onRemove={() => undefined}
                    />
                    <Row>
                        <Action tone="on" label="Сохранить" busy={busy} onClick={() => void run({ url: url.trim(), key: key.trim() || undefined })} />
                    </Row>
                </div>
            )}
            {message && <Line tone={message.ok ? 'ok' : 'error'}>{message.text}</Line>}
        </div>
    );
}

/** Шлюз на колонке поставлен вручную (пакет YAM, `duowave station install`): адрес и ключ пары из его .env. */
/**
 * «Шлюз уже на колонке»: компьютер становится её помощником. По паролю пульта колонки — ключ пары она отдаёт сама
 * (docs/plans/desk.md, этап 4); ключ пары руками — запасной путь для шлюза, поставленного вручную со своим PEER_KEY.
 */
export function LinkSheet({ onClose, onLinked, initialAddress }: { onClose: () => void; onLinked: () => void; initialAddress?: string }) {
    const [address, setAddress] = useState(initialAddress ?? '');
    const [port, setPort] = useState('8790');
    const [byKey, setByKey] = useState(false);
    const [password, setPassword] = useState('');
    const [name, setName] = useState('');
    const [key, setKey] = useState('');
    const [busy, setBusy] = useState(false);
    const [error, setError] = useState<string | null>(null);
    const link = async () => {
        setBusy(true);
        setError(null);
        try {
            if (byKey) await api.linkStation(address.trim(), Number(port) || 8790, key.trim());
            else await api.pairStation(address.trim(), Number(port) || 8790, password, name.trim());
            onLinked();
        } catch (e) {
            setError(problemOf(e));
        } finally {
            setBusy(false);
        }
    };
    const ready = address.trim() && (byKey ? key.trim().length >= 16 : password.length > 0);
    return (
        <Sheet
            title="Помогать колонке"
            about="Шлюз уже работает на колонке: этот компьютер станет её помощником — слух, голос и модель, копии настроек, обновления, мастерская. Без SSH."
            onClose={onClose}
            footer={
                <button class="dash-done" disabled={busy || !ready} onClick={() => void link()}>
                    {busy ? 'Связываюсь…' : 'Стать помощником'}
                </button>
            }
        >
            <TextField label="Адрес колонки" value={address} placeholder="192.0.2.23" inputMode="url" onInput={setAddress} />
            {byKey ? (
                <>
                    <TextField label="Порт шлюза на колонке" value={port} placeholder="8790" inputMode="numeric" onInput={setPort} />
                    <SecretField
                        label="Ключ пары"
                        value={key}
                        removing={false}
                        hint="PEER_KEY из /data/opt/duowave/gateway/.env на колонке (duowave station install --peer-key …)."
                        onInput={setKey}
                        onRemove={() => undefined}
                    />
                </>
            ) : (
                <>
                    <TextField label="Имя в пульте колонки" value={name} placeholder="если там не один человек" onInput={setName} />
                    <SecretField
                        label="Пароль пульта колонки"
                        value={password}
                        removing={false}
                        hint="Пароль администратора её пульта: компьютер войдёт один раз, возьмёт ключ пары и забудет пароль."
                        onInput={setPassword}
                        onRemove={() => undefined}
                    />
                </>
            )}
            <button class="t-caption underline cursor-pointer self-start" onClick={() => setByKey(!byKey)}>
                {byKey ? 'Связать по паролю пульта колонки' : 'Есть ключ пары — связать по нему'}
            </button>
            {error && <Line tone="error">{error}</Line>}
        </Sheet>
    );
}

function CopiesSheet({
    station,
    onClose,
    onRestore,
    onBackup,
}: {
    station: StationGatewayView;
    onClose: () => void;
    onRestore: (id: string) => Promise<void>;
    onBackup: () => Promise<void>;
}) {
    return (
        <Sheet title="Копии настроек колонки" about="Компьютер берёт копию, как только колонка назовёт новую ревизию; последние десять здесь." onClose={onClose}>
            <Row>
                <Action label="Взять копию сейчас" onClick={() => void onBackup()} />
            </Row>
            {station.backups.length ? (
                station.backups.map((copy) => (
                    <div key={copy.id} class="flex items-center gap-8 justify-between">
                        <span class="t-caption">
                            {clock(copy.at)} · ревизия {copy.revision} · {Math.max(1, Math.round(copy.size / 1024))} КБ
                        </span>
                        <Action
                            label="Восстановить"
                            onClick={async () => {
                                const yes = await ask({
                                    title: `Вернуть колонке настройки от ${clock(copy.at)}?`,
                                    text: 'Шлюз на колонке перезапустится на них.',
                                    ok: 'Вернуть',
                                });
                                if (yes) void onRestore(copy.id);
                            }}
                        />
                    </div>
                ))
            ) : (
                <Line>Копий пока нет.</Line>
            )}
        </Sheet>
    );
}

function InspectSheet({ inspected, onClose }: { inspected: Inspected; onClose: () => void }) {
    return (
        <Sheet title="Что с шлюзом на колонке" about="Колонка сама о себе, по SSH." onClose={onClose}>
            {!inspected.installed && <Line tone="warn">Шлюза на колонке нет: её сбросили или он удалён. «Сменить…» поставит его заново.</Line>}
            {inspected.installed && (
                <Line>
                    {inspected.running
                        ? 'Шлюз запущен.'
                        : inspected.disabled
                          ? 'Шлюз выключен страховкой: «Попробовать снова» включит его.'
                          : 'Шлюз не запущен.'}{' '}
                    {inspected.local ? 'Голос колонки идёт в него.' : 'Голос колонки идёт не в него.'}
                </Line>
            )}
            {inspected.fired && <Line tone="warn">Страховка: {inspected.fired}.</Line>}
            {inspected.log && (
                <pre class="t-meta pre-wrap max-h-40vh overflow-auto m-0">
                    {inspected.log}
                </pre>
            )}
        </Sheet>
    );
}
