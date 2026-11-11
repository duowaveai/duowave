import type { PanelMode } from './api';
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
    /** «Сейчас отвечает DeepSeek». */
    title: string;
}

const toneOf = (host: Host | null): WhoTone => (host === null ? 'off' : host === 'cloud' ? 'cloud' : host === 'yandex' ? 'yandex' : 'own');

const ALICE_EAR = (why: string): Who => ({ host: 'yandex', value: 'Яндекс', sub: why, tone: 'yandex' });
const ALICE_BRAIN: Who = { host: 'yandex', value: 'Алиса', sub: 'думает Яндекс', tone: 'yandex' };
const ALICE_VOICE: Who = { host: 'yandex', value: 'Алиса', sub: 'голос Яндекса', tone: 'yandex' };

export const MODE_TITLES: Record<PanelMode, string> = { relay: 'Яндекс', gateway: 'Вместе', own: 'Своё' };

export function solve(s: Scenario): Solved {
    const gateway: Host = s.placement === 'pc' ? 'pc' : 'station';
    const pcHelps = s.placement !== 'station' && s.pcUp;
    const notes: string[] = [];
    const nobody: Who = { host: null, value: 'никто', sub: 'компьютер спит', tone: 'off' };

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
            notes: ['Компьютер спит или выключен — колонке некуда отправить голос. Колонка молчит, пока компьютер не проснётся.'],
            title: 'Колонка молчит',
        };
    }

    let mode = s.mode;
    if (mode === 'own' && !pcHelps) {
        mode = 'gateway';
        notes.push(
            s.placement === 'station'
                ? '«Своё» на одной колонке не работает: ему нужен компьютер. Колонка работает в режиме «Вместе».'
                : 'Компьютер не на связи — колонка сама перешла из «Своего» в «Вместе». Вернётся, когда компьютер проснётся.',
        );
    }

    const yandexWhole = (why: string, note?: string): Solved => {
        if (note) notes.push(note);
        return {
            mode,
            gateway,
            silent: false,
            ear: ALICE_EAR(why),
            brain: ALICE_BRAIN,
            voice: ALICE_VOICE,
            chosenAnswers: s.brain.where === 'yandex',
            hops: [{ from: 'station', to: 'yandex', back: true, tone: 'yandex', text: 'Голос уходит Яндексу, отвечает Алиса' }],
            notes,
            title: 'Сейчас отвечает Алиса',
        };
    };

    if (mode === 'relay') return yandexWhole('режим «Яндекс»');

    const pcEar = pcHelps && s.pcEar !== false;
    const earHost: Host | null = pcEar ? 'pc' : s.cloudEar ? 'cloud' : null;
    if (!earHost) {
        const chosen = s.brain.where === 'cloud' || s.brain.where === 'pc' ? `${s.brain.name} в разговоре не участвует. ` : '';
        const fix = s.placement === 'pc' ? 'Запустите слух на компьютере.' : s.placement === 'station' ? 'Чтобы отвечал ваш мозг, подключите облачный слух или позовите на помощь компьютер.' : 'Чтобы отвечал ваш мозг, подключите облачный слух или разбудите компьютер.';
        return yandexWhole('слух не подключён', `Слух: нет → вопросы слышит Яндекс, и отвечает тоже он. ${chosen}${fix}`);
    }

    const hops: Hop[] = [];
    if (gateway === 'pc') hops.push({ from: 'station', to: 'pc', tone: 'own', text: 'Колонка отправляет голос в DuoWave на компьютере' });
    else hops.push({ from: 'station', to: 'station', tone: 'own', text: 'Голос принимает DuoWave на колонке' });

    const ear: Who =
        earHost === 'pc'
            ? { host: 'pc', value: 'Компьютер', sub: 'свой слух', tone: 'own' }
            : { host: 'cloud', value: 'Облако', sub: 'облачное распознавание', tone: 'cloud' };
    hops.push(
        earHost === gateway
            ? { from: gateway, to: gateway, tone: 'own', text: 'Компьютер распознаёт речь' }
            : { from: gateway, to: earHost, back: true, tone: toneOf(earHost), text: earHost === 'pc' ? 'Компьютер распознаёт речь' : 'Облако распознаёт речь' },
    );

    let brain: Who;
    let brainHost: Host | null = null;
    if (s.brain.where === 'yandex') brain = { ...ALICE_BRAIN, sub: 'разговор отдан Алисе в маршрутах' };
    else if (s.brain.where === 'none') brain = { host: null, value: 'нет', sub: 'мозг не выбран', tone: 'off' };
    else if (s.brain.where === 'pc' && !pcHelps) {
        brain = { ...ALICE_BRAIN, sub: 'модель на компьютере недоступна' };
        notes.push(`Выбрана ${s.brain.name}, а компьютер не на связи. На вопросы отвечает Алиса.`);
    } else {
        brainHost = s.brain.where === 'pc' ? 'pc' : 'cloud';
        brain = { host: brainHost, value: s.brain.name, sub: brainHost === 'pc' ? 'модель дома' : 'облако', tone: toneOf(brainHost) };
    }
    if (brainHost) {
        hops.push(
            brainHost === gateway
                ? { from: gateway, to: gateway, tone: 'own', text: `${s.brain.name} придумывает ответ` }
                : { from: gateway, to: brainHost, back: true, tone: toneOf(brainHost), text: `${s.brain.name} придумывает ответ` },
        );
    } else if (brain.host === 'yandex') {
        hops.push({ from: 'station', to: 'yandex', back: true, tone: 'yandex', text: 'Вопрос отвечает Алиса' });
    }

    let voice: Who;
    if (brain.host === null) {
        voice = { host: null, value: '—', sub: 'отвечать нечего', tone: 'off' };
        notes.push('Мозг не выбран: выберите его в «Персонаж» → «Мозг».');
    } else if (brain.host === 'yandex') voice = ALICE_VOICE;
    else if (pcHelps && s.pcVoice !== false) voice = { host: 'pc', value: 'Свой голос', sub: 'синтез на компьютере', tone: 'own' };
    else if (s.cloudVoice) voice = { host: 'cloud', value: 'Облако', sub: 'облачный голос', tone: 'cloud' };
    else voice = { host: 'yandex', value: 'Голос Алисы', sub: 'текст ваш, озвучивает Яндекс', tone: 'yandex' };
    if (brain.host !== 'yandex' && brain.host !== null) {
        if (voice.host === 'pc')
            hops.push(
                gateway === 'pc'
                    ? { from: 'pc', to: 'station', tone: 'own', text: 'Ответ своим голосом звучит из колонки' }
                    : { from: 'station', to: 'pc', back: true, tone: 'own', text: 'Компьютер озвучивает ответ своим голосом' },
            );
        else if (voice.host === 'cloud') hops.push({ from: gateway, to: 'cloud', back: true, tone: 'cloud', text: 'Облако озвучивает ответ' });
        else hops.push({ from: 'station', to: 'yandex', back: true, tone: 'yandex', text: 'Яндекс озвучивает готовый текст голосом Алисы' });
    }
    if (mode === 'own' && voice.host !== 'pc' && voice.host !== 'cloud') notes.push('В «Своём» голоса Алисы нет: без своего голоса ответ не прозвучит.');

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
        title: brain.host === null ? 'Ответить некому' : `Сейчас отвечает ${brain.value}`,
    };
}
