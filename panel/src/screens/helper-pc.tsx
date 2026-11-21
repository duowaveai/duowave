// Своё у компьютера-Турбо (docs/plans/home-and-helpers.md, блок 1, этап 8; прежде «Этот компьютер» и «Компьютер-
// помощник»): кому он помогает, ключ подключения, не засыпать; ниже — присмотр за колонкой, обновления и модели этого
// компьютера. Со страницей «Дом» (решение владельца 16.11) это её часть у пульта Турбо (screens/helpers.tsx). Когда компьютер помогает колонке, окно приложения — пульт колонки:
// всё в нём идёт к ней через проход компьютера. Своё у компьютера — эта страница: она говорит с шлюзом компьютера
// напрямую, поэтому в пульте колонки стоит фреймом без рамки (`?own=helper`, `?own=workshop`). Колонка не отвечает —
// страница всё равно работает.

import { useEffect, useMemo, useRef, useState } from 'preact/hooks';

import { ApiError, api, can, problemOf, sessionStore } from '../api';
import type { Ability, ClientView, GatewayView, HelperKeyView, HelperPart, HelperServed, PlacedStation, StationGatewayView } from '../api';
import { ABILITIES, ABILITY_ICONS, abilityAbout, abilityTitle, connectionText, readHelpers, useHelperKeyTurn, useHelpers } from '../helpers';
import { tr } from '../i18n';
import { usePoll } from '../poll';
import { usePanel } from '../shell/context';
import { useDevMode } from '../shell/dev';
import { helpsSomeone } from '../shell/role';
import { clock, since } from '../time';
import { ask } from '../ui/confirm';
import { TextField } from '../ui/fields';
import { encodeQr, qrPath } from '../ui/qr';
import { SaveBar, Sheet, SkeletonRows, WinRow, WinSection } from '../ui/sheet';
import { Switch } from '../ui/switch';
import { PopMenu } from '../ui/widget';
import { Command } from './connect';
import { Watch, usePlacementView } from './desk-widgets';
import { Updates } from './house';
import { drawingOf } from '../stations';
import { IconChevronDown, IconHome } from '../icons';
import { ModelsScreen } from './models';
import { PlacementCard } from './placement';
import { seconds } from './trip';
import { VoiceHearingBrief } from './voice-brief';
import { WorkshopScreen } from './workshop';

/**
 * Страница компьютера внутри фрейма: «helper» — всё своё у компьютера, «computer» — вкладка «Этот компьютер» его
 * страницы в «Устройствах» (силы, модели, присмотр), «homes» — «Другие дома» (кому он помогает, ключ), «workshop» —
 * мастерская (docs/plans/home-and-devices.md, этап 1).
 */
export function OwnPage({ page }: { page: string }) {
    // Фрейм сливается со страницей: прозрачный фон, без полей окна.
    useEffect(() => {
        document.documentElement.classList.add('own-page');
        return () => document.documentElement.classList.remove('own-page');
    }, []);
    // «voice», «hearing» — модели компьютера в пунктах «Настроек» «Голос» и «Слух» (screens/voice-home.tsx).
    const body =
        page === 'workshop' ? (
            <WorkshopScreen />
        ) : page === 'homes' ? (
            <HelperSide gives={false} />
        ) : page === 'computer' ? (
            <ThisComputer />
        ) : page === 'voice' ? (
            <ModelsScreen tab="voice" onTab={() => undefined} bare page />
        ) : page === 'hearing' ? (
            <ModelsScreen tab="recognition" onTab={() => undefined} bare page />
        ) : (
            <HelperComputer />
        );
    return <div class="own-page-body">{body}</div>;
}

/** «Мои силы»: что этот компьютер даёт дому как Турбо — готово или нет. */
export function TurboPowers() {
    const { view, error } = useHelpers();
    const builtin = view?.helpers.find((helper) => helper.builtin);
    const powers = (builtin?.abilities ?? []).filter((ability) => ability.id in ABILITY_ICONS);
    return (
        <Card id="powers" title={tr.helpers.powersTitle}>
            {powers.length === 0 ? (
                <p class="w-cap m-0 wrap-anywhere" role={!view && error ? 'alert' : undefined}>
                    {view ? tr.helpers.powersNone : (error ?? tr.helpers.reading)}
                </p>
            ) : (
                <div class="w-rows">
                    {powers.map((ability) => {
                        const Icon = ABILITY_ICONS[ability.id];
                        const details = ability.details;
                        const about = [details?.model, details?.voice, details?.languages?.join(', ')].filter(Boolean).join(' · ') || abilityAbout(ability.id);
                        return (
                            <div key={ability.id} class="w-row">
                                <Icon size={16} />
                                <span class="flex-1 min-w-0 flex flex-col">
                                    <span class="w-strong">{abilityTitle(ability.id)}</span>
                                    <span class="w-cap wrap-anywhere">{about}</span>
                                </span>
                                <span class={`pill ${ability.ready ? 'pill-own' : 'pill-off'}`}>{ability.ready ? tr.helpers.powersReady : tr.helpers.powersNotReady}</span>
                            </div>
                        );
                    })}
                </div>
            )}
        </Card>
    );
}

/** «Этот компьютер» на странице устройства: его силы, «не давать спать», модели слуха и голоса, присмотр за колонкой. */
export function ThisComputer() {
    return (
        <>
            <TurboPowers />
            <KeepAwake />
            <HelperComputer home side={false} />
            {isAdmin() && <DeveloperReport />}
        </>
    );
}

/**
 * «Отчёт для разработчика»: что-то не так на этом компьютере — отчёт копируется и уходит нам (мы чиним чужие
 * компьютеры по присланным журналам: тест на Windows 21.11). Секреты, домашняя папка, имя и почта из него убраны.
 */
function DeveloperReport() {
    const t = tr.helperPc;
    const [text, setText] = useState<string | null>(null);
    const [busy, setBusy] = useState(false);
    const [note, setNote] = useState<string | null>(null);
    const collect = async (): Promise<void> => {
        setBusy(true);
        setNote(null);
        try {
            const report = await api.developerReport();
            setText(report.text);
            try {
                await navigator.clipboard.writeText(report.text);
                setNote(t.reportCopied);
            } catch {
                setNote(t.reportSelect);
            }
        } catch (e) {
            setNote(problemOf(e));
        } finally {
            setBusy(false);
        }
    };
    return (
        <Card id="report" title={t.reportTitle}>
            <p class="w-cap m-0">{t.reportAbout}</p>
            <div>
                <button class="w-btn" disabled={busy} aria-busy={busy || undefined} onClick={() => void collect()}>
                    {t.reportCollect}
                </button>
            </div>
            {note && (
                <p class="w-cap m-0" role="status">
                    {note}
                </p>
            )}
            {text !== null && (
                <textarea class="field" readOnly rows={8} aria-label={t.reportTitle} value={text} onFocus={(event) => (event.target as HTMLTextAreaElement).select()} />
            )}
        </Card>
    );
}

function Card({ id, title, children }: { id: string; title: string; children: preact.ComponentChildren }) {
    return (
        <section class="card glass px-16 pt-4 pb-14 flex flex-col gap-10" aria-labelledby={`pc-${id}`}>
            <div class="section-head">
                <h2 id={`pc-${id}`} class="t-title-xs m-0">
                    {title}
                </h2>
            </div>
            {children}
        </section>
    );
}

const isAdmin = (): boolean => sessionStore.get()?.role === 'admin';

/**
 * Своё у компьютера: помощник других шлюзов, присмотр за колонкой, обновления, а глубже — его модели. С `?sheet=placement`
 * (из «Помощников» колонки, чьи настройки ещё на компьютере) присмотр открыт и стоит первым: там «Вернуть настройки на
 * колонку».
 */
export function HelperComputer({ home = false, side = true }: { home?: boolean; side?: boolean } = {}) {
    // «Открыть» у присмотра: всё о размещении — здесь же, глазами компьютера (копии, перенос, что случилось).
    const [first] = useState(() => new URLSearchParams(location.search).get('sheet') === 'placement');
    const [placement, setPlacement] = useState(first);
    const watch = can('station.view') && (
        <>
            <Card id="watch" title={tr.helperPc.watchTitle}>
                <Watch opened={placement} onOpen={() => setPlacement((open) => !open)} />
            </Card>
            {placement && <PlacementCard />}
        </>
    );
    return (
        <>
            {first && watch}
            {/* In «Дом» its powers stand in a card of their own above: here — who it helps. Updates have their page. */}
            {side && <HelperSide gives={!home} />}
            {!first && watch}
            {!home && <Updates />}
            {/* Слух и голос: на странице устройства — кратко, со ссылками в свои пункты «Настроек» (проверка начисто
                19.11: выбор моделей был спрятан здесь); глубже, у самого компьютера, — по одному разделу (холст «Окна
                DuoWave», 17.11). */}
            {can('models.manage') &&
                (home ? (
                    <VoiceHearingBrief />
                ) : (
                    <>
                        <ModelsScreen tab="recognition" onTab={() => undefined} bare />
                        <ModelsScreen tab="voice" onTab={() => undefined} bare />
                    </>
                ))}
        </>
    );
}

/** Today's count of an old part in its own words: «214 вопросов · 1,2 с». */
const servedWords = (part: HelperPart, served: HelperServed | null): string | null => {
    const today = served?.[part];
    if (!today || today.count === 0) return null;
    const count = { asr: tr.desk.questionsCount, tts: tr.desk.phrasesCount, llm: tr.desk.answersCount, messages: tr.desk.messagesCount }[part](today.count);
    return `${count} · ${seconds(today.avgMs)}`;
};

/** The old part names of `/api/v1/home-server/served` by ability. */
const PART_OF: Partial<Record<Ability, HelperPart>> = { hearing: 'asr', voice: 'tts', model: 'llm', messages: 'messages' };

/**
 * «Этот компьютер — помощник»: the name gateways see it by, «Даю» (the built-in helper's abilities), «Помогаю» (the
 * speakers it places and every gateway connected by a key), «Ключ подключения» for an admin, «Не давать компьютеру
 * спать» while it helps someone. Opened with `?sheet=key` (the station window's «Подключить шлюз»), the key sheet opens
 * at once.
 */
export function HelperSide({ bare = false, gives: showGives = true }: { bare?: boolean; gives?: boolean }) {
    const panel = usePanel();
    const dev = useDevMode();
    const { view, error: unread } = useHelpers();
    const [placement, reread] = usePlacementView();
    const [clients, setClients] = useState<ClientView[] | null>(null);
    const [served, setServed] = useState<HelperServed | null>(null);
    const [busy, setBusy] = useState<string | null>(null);
    const [error, setError] = useState<string | null>(null);
    const [sheet, setSheet] = useState<'key' | 'name' | ClientView | null>(() =>
        new URLSearchParams(location.search).get('sheet') === 'key' && isAdmin() ? 'key' : null,
    );
    usePoll({
        every: 30_000,
        version: panel.version,
        read: (signal) =>
            api
                .helperClients(signal)
                .then((answer) => !signal.aborted && setClients(answer.clients))
                .catch((e) => !signal.aborted && e instanceof ApiError && e.status === 404 && setClients(null)),
    });
    usePoll({ on: dev, every: 30_000, version: panel.version, read: (signal) => api.helperServed(signal).then((fresh) => !signal.aborted && setServed(fresh)) });
    const run = (what: string, job: () => Promise<unknown>): void => {
        setBusy(what);
        setError(null);
        void job()
            .then(() => reread())
            .catch((e) => setError(problemOf(e)))
            .finally(() => setBusy(null));
    };
    const builtin = view?.helpers.find((helper) => helper.builtin);
    const name = view?.gateway?.name ?? panel.health?.gateway?.name;
    const health = panel.health?.placement;
    const stations: PlacedStation[] = (health?.stations ?? (health?.station ? [health.station] : [])).filter((entry) => entry.placement !== 'pc');
    const others = (clients ?? []).filter((client) => !client.station || !stations.some((entry) => entry.key === client.station));
    const gives = (builtin?.abilities ?? []).filter((ability) => ability.id in ABILITY_ICONS);
    const body = (
        <>
            {!bare && <p class="w-cap m-0">{tr.helpers.sideAbout}</p>}
            {name && (
                <div class="w-row">
                    <span class="flex-1 min-w-0 w-cap">{tr.helpers.seenAs(name)}</span>
                    {can('settings.edit') && (
                        <button class="w-btn" onClick={() => setSheet('name')}>
                            {tr.helpers.change}
                        </button>
                    )}
                </div>
            )}
            {showGives && <div class="flex flex-col gap-6">
                <span class="w-strong">{tr.helpers.gives}</span>
                {gives.length === 0 ? (
                    <p class="w-cap m-0 wrap-anywhere" role={!view && unread ? 'alert' : undefined}>
                        {view ? tr.helpers.givesNothing : (unread ?? tr.helpers.reading)}
                    </p>
                ) : (
                    <div class="helpers-chips">
                        {gives.map((ability) => {
                            const Icon = ABILITY_ICONS[ability.id];
                            const part = PART_OF[ability.id];
                            const count = dev && part ? servedWords(part, served) : null;
                            return (
                                <span key={ability.id} class={`pill ${ability.ready ? 'pill-own' : 'pill-off'}`}>
                                    <Icon size={14} />
                                    {abilityTitle(ability.id)}
                                    {count && ` · ${count}`}
                                </span>
                            );
                        })}
                    </div>
                )}
                {dev && served && <p class="w-cap m-0">{tr.desk.countToday}</p>}
            </div>}
            {/* Кому помогаю — явный список (владелец 16.11): плитка на каждый шлюз — картинка колонки по её модели, имя,
                модель и состояние, «Помогать»; под ней — что даю значками, а тонкая настройка свёрнута отдельной панелью. */}
            <section class="side-block" aria-label={tr.helpers.helps}>
                <div class="side-block-head">
                    <span class="w-strong">{tr.helpers.helps}</span>
                    {stations.length + others.length > 0 && <span class="w-cap">{stations.length + others.length}</span>}
                </div>
                {stations.length === 0 && others.length === 0 ? (
                    <p class="w-cap m-0">{tr.helpers.helpsNone}</p>
                ) : (
                    <ul class="turbo-clients">
                        {stations.map((entry) => {
                            const gateway = [placement?.station, ...(placement?.stations ?? [])].find((one) => one?.key === entry.key);
                            const helping = entry.placement === 'stationServer';
                            const client = clients?.find((one) => one.station === entry.key);
                            const known = (panel.stations ?? []).find((one) => one.device === entry.device);
                            const drawing = drawingOf(known?.look, known?.deviceModel);
                            const { Glyph } = drawing;
                            // A speaker linked without a name: «Колонка» and its address, never its key («at:…»).
                            const title = entry.name ?? tr.helpers.stationUnnamed;
                            const about = [drawing.title, entry.name ? null : entry.address].filter(Boolean).join(' · ');
                            const on = helping && entry.reachable;
                            const state = helping ? (entry.reachable ? (client ? seenWords(client) : null) ?? tr.helpers.helpsOnline : tr.helpers.helpStationAway) : tr.helpers.helpStationOff;
                            // A 0.11–0.13 speaker reads no list of helpers: its four old part switches, in the developer layer.
                            const older = !!gateway?.live.health?.placement && !gateway.live.health.placement.helpers;
                            const without = gateway?.live.health?.placement?.homeServer?.without ?? [];
                            const taken = parts().filter((part) => !without.includes(part.id));
                            return (
                                <li key={entry.key} class="turbo-client">
                                    <div class="turbo-client-row">
                                        <span class="turbo-client-pic" aria-hidden="true">
                                            <Glyph color={drawing.color} size={30} />
                                        </span>
                                        <span class="turbo-client-text">
                                            <b>{title}</b>
                                            <span class="w-cap">{about}</span>
                                        </span>
                                        <span class={`turbo-client-state${on ? ' is-on' : ''}`}>
                                            <i aria-hidden="true" />
                                            {state}
                                        </span>
                                        <Switch
                                            on={helping}
                                            label={`${tr.helpers.helpStation}: ${title}`}
                                            disabled={!can('stations.manage') || busy !== null || !entry.reachable}
                                            onChange={(next) => run(entry.key, () => api.setStationHelper(next, entry.key))}
                                        />
                                    </div>
                                    {helping && (
                                        <div class="turbo-client-gives">
                                            <span class="w-cap">{tr.helpers.gives}</span>
                                            {taken.map((part) => (
                                                <span key={part.id} class="pill pill-own">
                                                    {part.title}
                                                </span>
                                            ))}
                                        </div>
                                    )}
                                    {dev && older && gateway && helping && <PartSwitches station={gateway} served={served} busy={busy !== null} run={run} />}
                                </li>
                            );
                        })}
                        {others.map((client) => {
                            const names = client.gateways.map((gateway) => gateway.name).join(', ') || client.name;
                            const online = client.gateways.some((gateway) => gateway.online);
                            return (
                                <li key={client.id} class="turbo-client">
                                    <div class="turbo-client-row">
                                        <span class="turbo-client-pic" aria-hidden="true">
                                            <IconHome size={20} />
                                        </span>
                                        <span class="turbo-client-text">
                                            <b>{names}</b>
                                            <span class="w-cap">{client.ownHome ? tr.helpers.clientOwnHome : tr.helpers.clientOtherHome}</span>
                                        </span>
                                        <span class={`turbo-client-state${online ? ' is-on' : ''}`}>
                                            <i aria-hidden="true" />
                                            {seenWords(client) ?? tr.helpers.helpStationAway}
                                        </span>
                                        {client.via === 'link' && can('connectors.manage') && (
                                            <PopMenu
                                                label={tr.helpers.clientMenu(names)}
                                                items={[
                                                    { label: tr.helpers.clientGives, act: () => setSheet(client) },
                                                    {
                                                        label: tr.helpers.clientRemove,
                                                        act: () =>
                                                            void ask({ title: tr.helpers.clientRemoveTitle(names), text: tr.helpers.clientRemoveText, ok: tr.helpers.clientRemove, danger: true }).then(
                                                                (yes) => yes && run(client.id, () => api.removeClient(client.id).then((answer) => setClients(answer.clients))),
                                                            ),
                                                    },
                                                ]}
                                            />
                                        )}
                                    </div>
                                    {client.gives.length > 0 && (
                                        <div class="turbo-client-gives">
                                            <span class="w-cap">{tr.helpers.gives}</span>
                                            {client.gives.map((id) => (
                                                <span key={id} class="pill pill-own">
                                                    {abilityTitle(id)}
                                                </span>
                                            ))}
                                        </div>
                                    )}
                                </li>
                            );
                        })}
                    </ul>
                )}
            </section>
            <section class="side-block" aria-label={tr.helpers.keyTitle}>
                <span class="w-strong">{tr.helpers.keyTitle}</span>
                {isAdmin() ? (
                    <>
                        <span class="w-cap">{tr.helpers.connectAbout}</span>
                        <div class="w-actions">
                            <button class="w-btn w-btn-ink" onClick={() => setSheet('key')}>
                                {tr.helpers.connect}
                            </button>
                        </div>
                    </>
                ) : (
                    <p class="w-cap m-0">{tr.helpers.keyAdminOnly}</p>
                )}
            </section>
            {/* Its words are about a speaker that leans on this computer: shown only while it helps someone — a speaker
                it helps («Помогать» on) or a gateway connected by a key —, not beside «работает сама», nor on a lone
                computer that is its own gateway. */}
            {helpsSomeone(stations, others.length) && (
                <section class="side-block">
                    <KeepAwake />
                </section>
            )}
            {error && (
                <p class="w-cap c-dot-offline m-0 wrap-anywhere" role="alert">
                    {error}
                </p>
            )}
            {sheet === 'key' && <KeySheet onClose={() => setSheet(null)} />}
            {sheet === 'name' && name && <GatewayNameSheet name={name} onClose={() => setSheet(null)} />}
            {typeof sheet === 'object' && sheet && <ClientSheet client={sheet} onClose={() => setSheet(null)} onSaved={setClients} />}
        </>
    );
    if (bare) return <div class="flex flex-col gap-10">{body}</div>;
    return (
        <Card id="side" title={tr.helpers.sideTitle}>
            {body}
        </Card>
    );
}

/** «на связи» when one of its gateways is, else when it was last seen. */
function seenWords(client: ClientView): string | null {
    if (client.gateways.some((gateway) => gateway.online)) return tr.helpers.helpsOnline;
    const last = client.gateways.map((gateway) => gateway.lastSeen ?? '').sort().pop();
    return last ? tr.helpers.helpsSeen(since(last)) : null;
}

/** «Что давать «…»»: the seven abilities for one home connected over the link. */
function ClientSheet({ client, onClose, onSaved }: { client: ClientView; onClose: () => void; onSaved: (clients: ClientView[]) => void }) {
    const [gives, setGives] = useState<Ability[]>(client.gives);
    const [busy, setBusy] = useState(false);
    const [error, setError] = useState<string | null>(null);
    const names = client.gateways.map((gateway) => gateway.name).join(', ') || client.name;
    const online = client.gateways.some((gateway) => gateway.online);
    const save = async (): Promise<void> => {
        setBusy(true);
        setError(null);
        try {
            onSaved((await api.setClientGives(client.id, gives)).clients);
            onClose();
        } catch (e) {
            setError(problemOf(e));
            setBusy(false);
        }
    };
    // The switches wait for «Сохранить», as before: nothing changed — no bar, the window closes by ✕.
    const dirty = gives.length !== client.gives.length || !gives.every((id) => client.gives.includes(id));
    return (
        <Sheet
            title={tr.helpers.clientGivesTitle(names)}
            meta={
                <>
                    <span class={`pill ${online ? 'pill-own' : 'pill-off'}`}>{seenWords(client) ?? tr.helpers.helpStationAway}</span>
                    <span class="win-chip">{client.ownHome ? tr.helpers.clientOwnHome : tr.helpers.clientOtherHome}</span>
                </>
            }
            onClose={onClose}
            footer={
                dirty || busy ? (
                    <SaveBar
                        dirty={dirty}
                        busy={busy}
                        saveLabel={tr.helpers.save}
                        onRevert={() => (setGives(client.gives), setError(null))}
                        onSave={() => void save()}
                    />
                ) : undefined
            }
        >
            <p class="win-lead">{tr.helpers.clientGivesAbout}</p>
            <div class="win-group">
                {ABILITIES.map((id) => {
                    const Icon = ABILITY_ICONS[id];
                    return (
                        <label key={id} class="win-row">
                            <span class="win-row-ico">
                                <Icon size={16} />
                            </span>
                            <span class="win-row-main">
                                <span class="win-row-t">{abilityTitle(id)}</span>
                            </span>
                            <Switch on={gives.includes(id)} label={abilityTitle(id)} onChange={(on) => setGives(on ? [...gives, id] : gives.filter((entry) => entry !== id))} />
                        </label>
                    );
                })}
            </div>
            {error && (
                <p class="win-callout m-0 wrap-anywhere" role="alert">
                    {error}
                </p>
            )}
        </Sheet>
    );
}

/** «214 вопросов», «198 фраз», «31 ответ» — слова и числа по языку; и чем колонка обходится без компьютера. */
export const parts = (): Array<{ id: HelperPart; title: string; without: string }> => [
    { id: 'asr', title: tr.desk.partEar, without: tr.desk.withoutCloud },
    { id: 'tts', title: tr.desk.partVoice, without: tr.desk.withoutAlice },
    { id: 'llm', title: tr.desk.partModel, without: tr.desk.withoutAlice },
    // Бот Telegram этого компьютера для колонки, в обе стороны: «Передай маме, что…» и сообщения контактов. У колонки
    // своего бота нет. Один переключатель на оба направления; счёт — тоже за оба.
    { id: 'messages', title: tr.desk.partMessages, without: tr.desk.withoutNobody },
];

/**
 * «Тонкая настройка» for a 0.11 speaker (developer layer): what it takes from this computer, part by part, with today's
 * count and what the speaker does without it. A new speaker has «Не брать у него…» on its own «Помощники» instead.
 */
function PartSwitches({ station, served, busy, run }: { station: StationGatewayView; served: HelperServed | null; busy: boolean; run: (what: string, job: () => Promise<unknown>) => void }) {
    const [open, setOpen] = useState(false);
    const helping = station.record?.placement === 'stationServer';
    const without = station.live.health?.placement?.homeServer?.without ?? [];
    const mayManage = can('stations.manage');
    return (
        <div class="turbo-fine">
            <button type="button" class="turbo-fine-toggle" aria-expanded={open} onClick={() => setOpen(!open)}>
                {tr.desk.fineTuning}
                <IconChevronDown size={14} />
            </button>
            {open && (
                <div class="turbo-fine-rows">
                    {parts().map((part) => {
                        const on = helping && !without.includes(part.id);
                        return (
                            <div key={part.id} class="turbo-fine-row">
                                <span class="turbo-fine-text">
                                    <span class="w-text">{part.title}</span>
                                    <span class="w-cap">
                                        {on ? (servedWords(part.id, served) ?? tr.desk.here) : tr.desk.nowWithout(part.without)} · {tr.desk.otherwise(part.without)}
                                    </span>
                                </span>
                                <Switch
                                    on={on}
                                    label={tr.desk.giveToStation(part.title.toLowerCase())}
                                    disabled={!mayManage || busy || !station.live.reachable || !helping}
                                    onChange={(next) =>
                                        run(part.id, () => api.setHelperParts(next ? without.filter((id) => id !== part.id) : [...without, part.id], station.key))
                                    }
                                />
                            </div>
                        );
                    })}
                </div>
            )}
        </div>
    );
}

/**
 * «Не давать компьютеру спать» (макет Main): уснёт — колонка не замолчит (слышит облако, говорит Алиса), но «Свой»
 * вернётся, только когда помощник проснётся. Настройка приложения DuoWave: в браузере вне его её нет.
 */
export function KeepAwake() {
    const [state, setState] = useState<{ managed: boolean; keepAwake?: boolean } | null>(null);
    const [busy, setBusy] = useState(false);
    useEffect(() => {
        let alive = true;
        void api
            .desktopAwake()
            .then((fresh) => alive && setState(fresh))
            .catch(() => alive && setState({ managed: false }));
        return () => {
            alive = false;
        };
    }, []);
    if (!state?.managed) return null;
    return (
        <label class="w-toggle">
            <span class="flex-1 min-w-0 flex flex-col">
                <span class="w-strong">{tr.desk.keepAwake}</span>
                <span class="w-cap">{tr.desk.keepAwakeNote}</span>
            </span>
            <Switch
                on={state.keepAwake !== false}
                label={tr.desk.keepAwake}
                disabled={busy || !can('settings.edit')}
                onChange={(on) => {
                    setBusy(true);
                    void api
                        .setDesktopAwake(on)
                        .then(setState)
                        .catch(() => undefined)
                        .finally(() => setBusy(false));
                }}
            />
        </label>
    );
}

/**
 * A name for a helper or for this gateway: one field; changed — «Есть изменения · Отменить · Сохранить» below (nothing
 * changed — nothing sent, Enter closes); the gateway's words on a refusal. `loading` — the name is still being read: the
 * window is already open, a grey line in the field's place.
 */
export function NameSheet({
    title,
    name,
    placeholder,
    hint,
    loading = false,
    onSave,
    onClose,
}: {
    title: string;
    name: string;
    placeholder?: string;
    hint?: string;
    loading?: boolean;
    onSave: (name: string) => Promise<void>;
    onClose: () => void;
}) {
    /** What is typed; untouched (null) — the name as read, which may come after the window opened. */
    const [typed, setTyped] = useState<string | null>(null);
    const [busy, setBusy] = useState(false);
    const [error, setError] = useState<string | null>(null);
    const value = typed ?? name;
    const dirty = !loading && value.trim() !== name.trim();
    const save = async (): Promise<void> => {
        if (loading) return;
        if (value.trim() === name.trim()) return onClose();
        setBusy(true);
        setError(null);
        try {
            await onSave(value.trim());
            onClose();
        } catch (e) {
            setError(problemOf(e));
            setBusy(false);
        }
    };
    return (
        <Sheet
            title={title}
            onClose={onClose}
            footer={
                dirty || busy ? (
                    <SaveBar dirty={dirty} busy={busy} saveLabel={tr.helpers.save} onRevert={() => (setTyped(null), setError(null))} onSave={() => void save()} />
                ) : undefined
            }
        >
            {loading ? (
                <SkeletonRows rows={1} />
            ) : (
                <form
                    onSubmit={(e) => {
                        e.preventDefault();
                        void save();
                    }}
                >
                    <TextField label={tr.helpers.nameField} value={value} placeholder={placeholder} hint={hint} onInput={setTyped} />
                </form>
            )}
            {error && (
                <p class="win-callout m-0 wrap-anywhere" role="alert">
                    {error}
                </p>
            )}
        </Sheet>
    );
}

/**
 * This gateway's name, as helpers see it (`/api/v1/gateway/self`): the field holds a person's own name, the default is
 * its hint. Saved empty or as the default, the name is the default again — it then follows the station's model once
 * known and the reader's language, which a default kept as a person's name would not. The window opens at once and
 * shows the field once the name is read; not read (no right to it) — the name as shown, kept as given.
 */
export function GatewayNameSheet({ name, onClose }: { name: string; onClose: () => void }) {
    const [self, setSelf] = useState<GatewayView | null>();
    useEffect(() => {
        let alive = true;
        api.gatewaySelf().then(
            (view) => alive && setSelf(view),
            () => alive && setSelf(null),
        );
        return () => {
            alive = false;
        };
    }, []);
    const fallback = self?.defaultName;
    return (
        <NameSheet
            title={tr.helpers.gatewayNameTitle}
            name={self ? (self.named ? self.name : '') : name}
            placeholder={fallback}
            hint={fallback && tr.helpers.gatewayNameDefault}
            loading={self === undefined}
            onSave={async (value) => {
                await api.renameGateway(value && value !== fallback ? value : null);
                // The answer is not the list: read it afresh, not the read already on its way from before the change.
                await readHelpers(true);
            }}
            onClose={onClose}
        />
    );
}

/** The QR of the connection text: black on white in every theme (cameras need it), 208 px. */
function QrImage({ text }: { text: string }) {
    const code = useMemo(() => encodeQr(text), [text]);
    if (!code) return null;
    const side = code.size + 8;
    return (
        <div class="qr-card">
            <svg viewBox={`0 0 ${side} ${side}`} width={208} height={208} shape-rendering="crispEdges" role="img" aria-label={tr.helpers.keyQr}>
                <rect width={side} height={side} fill="#fff" />
                <path d={qrPath(code)} fill="#000" />
            </svg>
        </div>
    );
}

/** How long a key is taken once «Подключить шлюз» is opened (the gateway's ARMED_FOR). */
const ARMED_MS = 15 * 60_000;

/**
 * «Подключить шлюз»: the connection key as text and QR (only for an admin). Opening it lets the key in for 15 minutes;
 * it is read again only when the key changed — a gateway that connected leaves a new one, or someone changed it — never
 * on the panel's other news, which would keep the key let in for as long as the sheet stays open. A key no longer
 * taken — the gateway sends no `armedUntil` (the address saved after the 15 minutes), or that time came while the sheet
 * stayed open — is not shown: a gateway given it would be refused; «Показать новый ключ» reads it again, as opening the
 * sheet does, and gets a new key, taken for 15 minutes. Without an address (in Docker the gateway sees none) the key
 * is shown alone, and the address is typed on the gateway. Warnings first.
 */
export function KeySheet({ onClose }: { onClose: () => void }) {
    const turn = useHelperKeyTurn();
    const t = tr.helpers;
    const [view, setView] = useState<HelperKeyView | null>(null);
    const [failure, setFailure] = useState<string | null>(null);
    const [address, setAddress] = useState<string | null>(null);
    const [outside, setOutside] = useState<string | null>(null);
    /** What is being changed: «Адрес снаружи» or the key itself. */
    const [busy, setBusy] = useState<'outside' | 'rotate' | null>(null);
    const [error, setError] = useState<string | null>(null);
    /** «Показать новый ключ»: one more read. */
    const [again, setAgain] = useState(0);
    /** The 15 minutes of the key shown came while the sheet stayed open. */
    const [expired, setExpired] = useState(false);
    /** A fresh view; the address picked stays while listed, unless `prefer` names the one to show now. */
    const take = (fresh: HelperKeyView, prefer?: string): void => {
        setView(fresh);
        setFailure(null);
        setExpired(false);
        setAddress((current) => {
            const shown = prefer ?? current;
            return shown && fresh.addresses.includes(shown) ? shown : (fresh.addresses[0] ?? null);
        });
    };
    useEffect(() => {
        let alive = true;
        api.helperKey()
            .then((fresh) => alive && take(fresh))
            .catch((e) => {
                if (!alive) return;
                const status = e instanceof ApiError ? e.status : 0;
                setFailure(status === 403 ? t.keyAdminOnly : status === 404 ? t.notHelper : problemOf(e));
            });
        return () => {
            alive = false;
        };
    }, [turn, again]);
    // The time the key stops being taken, by this device's clock: never later than 15 minutes from now (a clock behind
    // the gateway's), and 15 minutes when that time looks past already (a clock ahead) — the gateway said it is taken.
    useEffect(() => {
        if (!view?.armedUntil) return;
        const left = Date.parse(view.armedUntil) - Date.now();
        const timer = setTimeout(() => setExpired(true), left > 0 ? Math.min(left, ARMED_MS) : ARMED_MS);
        return () => clearTimeout(timer);
    }, [view]);
    // No address: the key alone, as the gateway's own payload is then.
    const text = !view ? '' : address ? connectionText(address, view.key) : view.key;
    /** `toOutside`: «Адрес снаружи» just saved is what the text and the QR carry now (a proxy's pairing needs it). */
    const change = async (what: 'outside' | 'rotate', job: () => Promise<HelperKeyView>, toOutside = false): Promise<void> => {
        setBusy(what);
        setError(null);
        try {
            const fresh = await job();
            take(fresh, toOutside ? fresh.outside : undefined);
            setOutside(null);
        } catch (e) {
            setError(problemOf(e));
        } finally {
            setBusy(null);
        }
    };
    const rotate = async (): Promise<void> => {
        if (await ask({ title: t.keyRotateTitle, text: t.keyRotateText, ok: t.keyRotateOk, danger: true })) await change('rotate', () => api.rotateHelperKey());
    };
    const typed = outside ?? view?.outside ?? '';
    /** «Адрес снаружи» typed and not saved: the bar below. */
    const dirty = !!view && typed.trim() !== (view.outside ?? '');
    /** The key shown is no longer taken: «Показать новый ключ» is the window's one action. */
    const stale = !!view && !view.problem && (expired || !view.armedUntil);
    const footer =
        dirty || busy === 'outside' ? (
            <SaveBar
                dirty={dirty}
                busy={busy === 'outside'}
                canSave={busy === null}
                saveLabel={t.keyOutsideSave}
                onRevert={() => setOutside(null)}
                onSave={() => void change('outside', () => api.setHelperOutside(typed.trim() || null), true)}
            />
        ) : stale ? (
            <button class="btn btn-main" disabled={busy !== null} onClick={() => setAgain((n) => n + 1)}>
                {t.keyNew}
            </button>
        ) : undefined;
    return (
        <Sheet
            title={t.keySheetTitle}
            meta={
                view && !view.problem ? (
                    view.armedUntil && !expired ? (
                        <span class="pill pill-own">{t.keyArmedChip(clock(view.armedUntil))}</span>
                    ) : (
                        <span class="pill pill-off">{t.keyStaleChip}</span>
                    )
                ) : undefined
            }
            onClose={onClose}
            footer={footer}
        >
            <p class="win-lead">{t.keySheetAbout}</p>
            {failure ? (
                <p class="win-callout m-0" role="alert">
                    {failure}
                </p>
            ) : !view ? (
                <SkeletonRows rows={3} label={t.reading} />
            ) : (
                <>
                    {/* Warnings first. */}
                    <p class="win-callout m-0">{t.keyCare}</p>
                    {view.problem ? (
                        <p class="win-callout m-0 wrap-anywhere" role="alert">
                            {view.problem}
                        </p>
                    ) : (
                        <>
                            {stale ? (
                                <p class="win-callout m-0" role="status">
                                    {t.keyExpired}
                                </p>
                            ) : (
                                <>
                                    {!address && <p class="win-callout m-0">{t.keyNoAddress}</p>}
                                    {view.addresses.length > 1 && (
                                        <label class="flex flex-col gap-6">
                                            <span class="win-label">{t.keyAddressPick}</span>
                                            <select class="field" value={address ?? ''} onChange={(e) => setAddress((e.target as HTMLSelectElement).value)}>
                                                {view.addresses.map((entry) => (
                                                    <option key={entry} value={entry}>
                                                        {entry}
                                                    </option>
                                                ))}
                                            </select>
                                        </label>
                                    )}
                                    <QrImage text={text} />
                                    {/* The connect sheet's field: selected whole, copied by a button (plain http: selected). */}
                                    <Command text={text} label={t.keyText} />
                                    <div class="win-group">
                                        {address && (
                                            <WinRow title={t.keyAddress}>
                                                <span class="win-code">{address}</span>
                                            </WinRow>
                                        )}
                                        <WinRow title={t.keyKey}>
                                            <span class="win-code">{view.key}</span>
                                        </WinRow>
                                    </div>
                                </>
                            )}
                            <p class="win-note">{t.seenAs(view.name)}</p>
                            {view.port && <p class="win-note">{t.keyPort(view.port)}</p>}
                            <p class="win-note">{t.keyChanges}</p>
                        </>
                    )}
                    <label class="flex flex-col gap-6">
                        <span class="win-label">{t.keyOutside}</span>
                        <input class="field" value={typed} placeholder="https://helper.example.com" inputMode="url" autocomplete="off" spellcheck={false} onInput={(e) => setOutside((e.target as HTMLInputElement).value)} />
                        <span class="win-hint">{t.keyOutsideAbout}</span>
                    </label>
                    {error && (
                        <p class="win-callout m-0 wrap-anywhere" role="alert">
                            {error}
                        </p>
                    )}
                    <WinSection title={tr.ui.danger} group>
                        <WinRow title={t.keyRotate} sub={t.keyRotateText}>
                            <button class="btn btn-danger btn-sm" disabled={busy !== null} aria-busy={busy === 'rotate' || undefined} onClick={() => void rotate()}>
                                {busy === 'rotate' && <span class="spinner spinner-sm" aria-hidden="true" />}
                                {t.keyRotateOk}
                            </button>
                        </WinRow>
                    </WinSection>
                </>
            )}
        </Sheet>
    );
}

/** Фрейм страницы компьютера в пульте колонки: без рамки, высотой с окно, прокрутка — внутри; `sheet` — что открыть сразу. */
export function OwnFrame({ page, title, sheet }: { page: 'helper' | 'workshop' | 'computer' | 'homes' | 'voice' | 'hearing'; title: string; sheet?: string }) {
    // Окно внутри страницы компьютера («Свой голос», мастерская) — во весь пульт: пока оно открыто, фрейм растягивается
    // на всё окно, а не жмёт окно в свои рамки (проверка начисто 18.11, п. 35). Источник тот же — смотрим прямо в него.
    const frame = useRef<HTMLIFrameElement>(null);
    const [covering, setCovering] = useState(false);
    useEffect(() => {
        const node = frame.current;
        if (!node) return;
        let watch: MutationObserver | null = null;
        const attach = (): void => {
            watch?.disconnect();
            const body = node.contentDocument?.body;
            if (!body) return;
            let held = false;
            const check = (): void => {
                const open = !!body.querySelector(':scope > .sheet-backdrop');
                const inner = node.contentDocument?.documentElement;
                // На весь пульт — только затемнение и окно: сама страница внутри фрейма остаётся там, где была, своей
                // ширины (проверка начисто 18.11, п. 39: растягивалась вместе с фреймом, подписи разъезжались к краям).
                if (open && !held && inner) {
                    const place = node.getBoundingClientRect();
                    const scrolled = node.contentWindow?.scrollY ?? 0;
                    inner.style.setProperty('--own-left', `${place.left}px`);
                    inner.style.setProperty('--own-top', `${place.top}px`);
                    inner.style.setProperty('--own-w', `${place.width}px`);
                    inner.style.setProperty('--own-h', `${place.height}px`);
                    // Видна была только часть фрейма — внутри прокрутки окна раздела: так и остаётся.
                    let seen = { top: 0, left: 0, bottom: innerHeight, right: innerWidth };
                    for (let up = node.parentElement; up; up = up.parentElement) {
                        if (!/auto|scroll|hidden/.test(getComputedStyle(up).overflowY)) continue;
                        const box = up.getBoundingClientRect();
                        seen = {
                            top: Math.max(seen.top, box.top),
                            left: Math.max(seen.left, box.left),
                            bottom: Math.min(seen.bottom, box.bottom),
                            right: Math.min(seen.right, box.right),
                        };
                    }
                    const cut = (value: number): string => `${Math.max(0, Math.round(value))}px`;
                    inner.style.setProperty(
                        '--own-clip',
                        `inset(${cut(seen.top - place.top)} ${cut(place.right - seen.right)} ${cut(place.bottom - seen.bottom)} ${cut(seen.left - place.left)})`,
                    );
                    inner.classList.add('own-held');
                    const page = node.contentDocument?.querySelector('.own-page-body');
                    if (page) page.scrollTop = scrolled;
                    held = true;
                } else if (!open && held && inner) {
                    const page = node.contentDocument?.querySelector('.own-page-body');
                    const scrolled = page?.scrollTop ?? 0;
                    inner.classList.remove('own-held');
                    node.contentWindow?.scrollTo(0, scrolled);
                    held = false;
                }
                setCovering(open);
                // Окно раздела, в котором стоит фрейм, — свой слой (15): растянутый фрейм остался бы под доком и
                // строкой. Пока окно компьютера открыто, слой окна раздела — над ними.
                document.documentElement.classList.toggle('own-covering', open);
            };
            watch = new MutationObserver(check);
            watch.observe(body, { childList: true });
            check();
        };
        node.addEventListener('load', attach);
        attach();
        return () => {
            node.removeEventListener('load', attach);
            watch?.disconnect();
            document.documentElement.classList.remove('own-covering');
        };
    }, []);
    return (
        <iframe
            ref={frame}
            class={`own-frame${covering ? ' is-covering' : ''}`}
            src={`${location.pathname}?own=${page}${sheet ? `&sheet=${encodeURIComponent(sheet)}` : ''}`}
            title={title}
        />
    );
}
