import type { PanelMode } from './api';
import { tr } from './i18n';
import type { Placement } from './availability';

/**
 * Кто на самом деле слышит, думает и говорит — при этом размещении, режиме и условиях (docs/plans/duowave-placement-ui.md,
 * решение 3). Один расчёт на схему окна «Где работает DuoWave» и на строку главного экрана: они не расходятся.
 *
 * - «Яндекс» — всё делает Яндекс.
 * - «Свой» без ПК (одна колонка, ПК спит) — на деле «Шлюз».
 * - «Шлюз» без слуха (ни ПК, ни облачного распознавания) — голос уходит Яндексу целиком, отвечает он; выбранный мозг
 *   не участвует (так делает шлюз, station_session.rs: «no recognition here»).
 * - Мозг «модель на ПК» без ПК — отвечает Алиса.
 * - Голос: ПК на связи — свой; иначе облачный, если есть; иначе голос Алисы читает наш текст.
 * - Шлюз на ПК, а ПК спит — колонка молчит.
 * - Режим «Микрофон» (`computer`, docs/plans/mic-mode.md): колонки и Яндекса нет вовсе. Нет слуха, голоса или мозга —
 *   так и сказано, и где их поставить (`fixes`); Алиса никого не подменяет.
 */

/** Где работает часть: колонка, ПК, облако, Яндекс. */
export type Host = 'station' | 'pc' | 'cloud' | 'yandex';
/** Цвет по DESIGN.md: лайм — своё (колонка, ПК), янтарь — облако и Яндекс, сланец — никто. */
export type WhoTone = 'own' | 'cloud' | 'yandex' | 'off';

export interface Scenario {
    placement: Placement;
    mode: PanelMode;
    /** ПК отвечает: при «На компьютере» — сам шлюз; при «Колонка + ПК» — домашний сервер на связи. */
    pcUp: boolean;
    /** На ПК работает распознавание и голос (на компьютере их сервер может молчать). */
    pcEar?: boolean;
    pcVoice?: boolean;
    cloudEar: boolean;
    cloudVoice: boolean;
    /** Выбранный мозг: его имя и где он работает; yandex — разговор отдан Алисе маршрутом; none — не выбран. */
    brain: { name: string; where: 'cloud' | 'pc' | 'yandex' | 'none' };
    /** Говорят микрофоном этого компьютера: колонки и Яндекса нет. */
    computer?: boolean;
}

/** Чего не хватает и где это поставить: строка и место пульта (раздел и вкладка). */
export interface Fix {
    text: string;
    place: [string, string];
    label: string;
}

export interface Who {
    host: Host | null;
    value: string;
    sub: string;
    tone: WhoTone;
}

/** Шаг пути голоса: откуда куда (туда и обратно — back), и что происходит. */
export interface Hop {
    from: Host;
    to: Host;
    back?: boolean;
    tone: WhoTone;
    text: string;
}

export interface Solved {
    /** Режим на деле: «Свой» без ПК — «Шлюз». */
    mode: PanelMode;
    /** Где живёт сам DuoWave. */
    gateway: Host;
    /** Колонка молчит: шлюз на ПК, а ПК спит. */
    silent: boolean;
    ear: Who;
    brain: Who;
    voice: Who;
    /** Выбранный мозг отвечает. */
    chosenAnswers: boolean;
    hops: Hop[];
    notes: string[];
    /** Режим «Микрофон»: чего не хватает и где это поставить. */
    fixes?: Fix[];
    /** «Сейчас отвечает DeepSeek». */
    title: string;
}

const toneOf = (host: Host | null): WhoTone => (host === null ? 'off' : host === 'cloud' ? 'cloud' : host === 'yandex' ? 'yandex' : 'own');

// Читаются при обращении (функции, не константы): язык пульта меняется без перезагрузки.
const aliceEar = (why: string): Who => ({ host: 'yandex', value: tr.core.yandex, sub: why, tone: 'yandex' });
const aliceBrain = (): Who => ({ host: 'yandex', value: tr.core.alice, sub: tr.core.aliceThinks, tone: 'yandex' });
const aliceVoice = (): Who => ({ host: 'yandex', value: tr.core.alice, sub: tr.core.aliceVoiceSub, tone: 'yandex' });

export const MODE_TITLES: Record<PanelMode, string> = {
    get relay() {
        return tr.core.modeRelay;
    },
    get gateway() {
        return tr.core.modeGateway;
    },
    get own() {
        return tr.core.modeOwn;
    },
};

/** Режим «Микрофон»: всё на этом компьютере или в облаке; чего нет — «нет», со ссылкой, где поставить. */
function solveComputer(s: Scenario): Solved {
    const fixes: Fix[] = [];
    const hops: Hop[] = [{ from: 'pc', to: 'pc', tone: 'own', text: tr.core.hopComputerMic }];
    const earHost: Host | null = s.pcEar !== false ? 'pc' : s.cloudEar ? 'cloud' : null;
    const ear: Who =
        earHost === 'pc'
            ? { host: 'pc', value: tr.core.whoComputer, sub: tr.core.whyOwnEar, tone: 'own' }
            : earHost === 'cloud'
              ? { host: 'cloud', value: tr.core.whoCloud, sub: tr.core.whyCloudEar, tone: 'cloud' }
              : { host: null, value: tr.core.whoNone, sub: tr.core.whyNoEar, tone: 'off' };
    if (earHost) hops.push({ from: 'pc', to: earHost, back: earHost !== 'pc', tone: toneOf(earHost), text: earHost === 'pc' ? tr.core.hopPcRecognizes : tr.core.hopCloudRecognizes });
    else fixes.push({ text: tr.core.fixNoEar, place: ['settings', 'station'], label: tr.core.fixNoEarLabel });

    const brainHost: Host | null = s.brain.where === 'pc' ? 'pc' : s.brain.where === 'cloud' ? 'cloud' : null;
    const brain: Who = brainHost
        ? { host: brainHost, value: s.brain.name, sub: brainHost === 'pc' ? tr.core.whyHomeModel : tr.core.whyCloud, tone: toneOf(brainHost) }
        : { host: null, value: tr.core.whoNone, sub: tr.core.whyNoBrain, tone: 'off' };
    if (brainHost) hops.push({ from: 'pc', to: brainHost, back: brainHost !== 'pc', tone: toneOf(brainHost), text: tr.core.hopThinks(s.brain.name) });
    else fixes.push({ text: tr.core.fixNoBrain, place: ['character', 'brain'], label: tr.core.fixNoBrainLabel });

    const voiceHost: Host | null = s.pcVoice !== false ? 'pc' : s.cloudVoice ? 'cloud' : null;
    const voice: Who =
        voiceHost === 'pc'
            ? { host: 'pc', value: tr.core.whoOwnVoice, sub: tr.core.whyPcSynth, tone: 'own' }
            : voiceHost === 'cloud'
              ? { host: 'cloud', value: tr.core.whoCloud, sub: tr.core.whyCloudVoice, tone: 'cloud' }
              : { host: null, value: tr.core.whoNone, sub: tr.core.whyNoVoice, tone: 'off' };
    if (voiceHost && brainHost) hops.push({ from: 'pc', to: voiceHost, back: voiceHost !== 'pc', tone: toneOf(voiceHost), text: tr.core.hopPcVoice });
    if (!voiceHost) fixes.push({ text: tr.core.fixNoVoice, place: ['character', 'voice'], label: tr.core.fixNoVoiceLabel });

    return {
        mode: 'own',
        gateway: 'pc',
        silent: false,
        ear,
        brain,
        voice,
        chosenAnswers: !!brainHost,
        hops,
        notes: fixes.map((fix) => fix.text),
        fixes,
        title: brainHost ? tr.core.titleAnswering(brain.value) : tr.core.titleNobody,
    };
}

export function solve(s: Scenario): Solved {
    if (s.computer) return solveComputer(s);
    const gateway: Host = s.placement === 'pc' ? 'pc' : 'station';
    const pcHelps = s.placement !== 'station' && s.pcUp;
    const notes: string[] = [];
    const nobody: Who = { host: null, value: tr.core.whoNobody, sub: tr.core.whyPcAsleep, tone: 'off' };

    if (s.placement === 'pc' && !s.pcUp) {
        return {
            mode: s.mode,
            gateway,
            silent: true,
            ear: nobody,
            brain: nobody,
            voice: nobody,
            chosenAnswers: false,
            hops: [],
            notes: [tr.core.notePcAsleep],
            title: tr.core.titleSilent,
        };
    }

    let mode = s.mode;
    if (mode === 'own' && !pcHelps) {
        mode = 'gateway';
        notes.push(
            s.placement === 'station'
                ? tr.core.noteOwnNeedsPc
                : tr.core.notePcOfflineSwitched,
        );
    }

    const yandexWhole = (why: string, note?: string): Solved => {
        if (note) notes.push(note);
        return {
            mode,
            gateway,
            silent: false,
            ear: aliceEar(why),
            brain: aliceBrain(),
            voice: aliceVoice(),
            chosenAnswers: s.brain.where === 'yandex',
            hops: [{ from: 'station', to: 'yandex', back: true, tone: 'yandex', text: tr.core.hopVoiceToYandex }],
            notes,
            title: tr.core.titleAliceAnswers,
        };
    };

    if (mode === 'relay') return yandexWhole(tr.core.whyRelayMode);

    const pcEar = pcHelps && s.pcEar !== false;
    const earHost: Host | null = pcEar ? 'pc' : s.cloudEar ? 'cloud' : null;
    if (!earHost) {
        const chosen = s.brain.where === 'cloud' || s.brain.where === 'pc' ? tr.core.brainNotInvolved(s.brain.name) : '';
        const fix = s.placement === 'pc' ? tr.core.fixEarPc : s.placement === 'station' ? tr.core.fixEarStation : tr.core.fixEarOther;
        return yandexWhole(tr.core.whyNoEarConnected, tr.core.noteNoEar(chosen, fix));
    }

    const hops: Hop[] = [];
    if (gateway === 'pc') hops.push({ from: 'station', to: 'pc', tone: 'own', text: tr.core.hopVoiceToPc });
    else hops.push({ from: 'station', to: 'station', tone: 'own', text: tr.core.hopVoiceToStation });

    const ear: Who =
        earHost === 'pc'
            ? { host: 'pc', value: tr.core.whoComputer, sub: tr.core.whyOwnEar, tone: 'own' }
            : { host: 'cloud', value: tr.core.whoCloud, sub: tr.core.whyCloudEar, tone: 'cloud' };
    hops.push(
        earHost === gateway
            ? { from: gateway, to: gateway, tone: 'own', text: tr.core.hopPcRecognizes }
            : { from: gateway, to: earHost, back: true, tone: toneOf(earHost), text: earHost === 'pc' ? tr.core.hopPcRecognizes : tr.core.hopCloudRecognizes },
    );

    let brain: Who;
    let brainHost: Host | null = null;
    if (s.brain.where === 'yandex') brain = { ...aliceBrain(), sub: tr.core.whyBrainGivenToAlice };
    else if (s.brain.where === 'none') brain = { host: null, value: tr.core.whoNone, sub: tr.core.whyNoBrain, tone: 'off' };
    else if (s.brain.where === 'pc' && !pcHelps) {
        brain = { ...aliceBrain(), sub: tr.core.whyModelUnavailable };
        notes.push(tr.core.notePcModelOffline(s.brain.name));
    } else {
        brainHost = s.brain.where === 'pc' ? 'pc' : 'cloud';
        brain = { host: brainHost, value: s.brain.name, sub: brainHost === 'pc' ? tr.core.whyHomeModel : tr.core.whyCloud, tone: toneOf(brainHost) };
    }
    if (brainHost) {
        hops.push(
            brainHost === gateway
                ? { from: gateway, to: gateway, tone: 'own', text: tr.core.hopThinks(s.brain.name) }
                : { from: gateway, to: brainHost, back: true, tone: toneOf(brainHost), text: tr.core.hopThinks(s.brain.name) },
        );
    } else if (brain.host === 'yandex') {
        hops.push({ from: 'station', to: 'yandex', back: true, tone: 'yandex', text: tr.core.hopAliceAnswers });
    }

    let voice: Who;
    if (brain.host === null) {
        voice = { host: null, value: tr.core.whoDash, sub: tr.core.whyNothingToAnswer, tone: 'off' };
        notes.push(tr.core.noteChooseBrain);
    } else if (brain.host === 'yandex') voice = aliceVoice();
    else if (pcHelps && s.pcVoice !== false) voice = { host: 'pc', value: tr.core.whoOwnVoice, sub: tr.core.whyPcSynth, tone: 'own' };
    else if (s.cloudVoice) voice = { host: 'cloud', value: tr.core.whoCloud, sub: tr.core.whyCloudVoice, tone: 'cloud' };
    else voice = { host: 'yandex', value: tr.core.whoAliceVoice, sub: tr.core.whyAliceVoice, tone: 'yandex' };
    if (brain.host !== 'yandex' && brain.host !== null) {
        if (voice.host === 'pc')
            hops.push(
                gateway === 'pc'
                    ? { from: 'pc', to: 'station', tone: 'own', text: tr.core.hopStationVoice }
                    : { from: 'station', to: 'pc', back: true, tone: 'own', text: tr.core.hopPcSpeaks },
            );
        else if (voice.host === 'cloud') hops.push({ from: gateway, to: 'cloud', back: true, tone: 'cloud', text: tr.core.hopCloudSpeaks });
        else hops.push({ from: 'station', to: 'yandex', back: true, tone: 'yandex', text: tr.core.hopYandexSpeaks });
    }
    if (mode === 'own' && voice.host !== 'pc' && voice.host !== 'cloud') notes.push(tr.core.noteOwnNoAliceVoice);

    const chosenAnswers = brain.host !== null && brain.host !== 'yandex' ? true : s.brain.where === 'yandex';
    return {
        mode,
        gateway,
        silent: false,
        ear,
        brain,
        voice,
        chosenAnswers,
        hops,
        notes,
        title: brain.host === null ? tr.core.titleNobody : tr.core.titleAnswering(brain.value),
    };
}
