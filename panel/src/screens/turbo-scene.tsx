// Второе тело сцены стола (решение владельца 16.11, доски DuoDesk, DuoAsleep, SpeakerDesk, MicDesk, HelperDesk): рядом с
// лёгким шлюзом (колонкой или компьютером) — тёмный блок Турбо с его силами и связью; нет Турбо у колонки — пустое
// место «Турбо нет · Добавить Турбо»; Турбо уснул — блок серый пунктиром и полоса «колонка в лёгком режиме». Под
// сценой — строка «Если Турбо уснёт: Думает — …, Слышит — …, Говорит — …». Что показать — turbo-model.ts.

import type { FunctionComponent } from 'preact';

import type { Ability } from '../api';
import { ABILITY_ICONS, abilityTitle, placeTitle } from '../helpers';
import { tr } from '../i18n';
import { IconBolt } from '../icons';
import { since } from '../time';
import type { FallItem, TurboView } from './turbo-model';

type Icon = FunctionComponent<{ size?: number }>;

/** Силы в блоке Турбо — как на доске: модель, слух, голос, тяжёлые приложения. */
const CHIPS: Ability[] = ['model', 'hearing', 'voice', 'apps'];

/** Связь шлюза с Турбо: сплошная — дома, пунктир — через интернет, внутри — встроенный; уснул — серый пунктир. */
function linkOf(turbo: TurboView): { cls: string; words: string } {
    if (turbo.builtin) return { cls: 'is-same', words: tr.dashboard.linkSame };
    const home = turbo.helper.kind === 'home';
    return { cls: `${home ? 'is-home' : 'is-net'}${turbo.up ? '' : ' is-off'}`, words: home ? tr.dashboard.linkHome : tr.dashboard.linkNet };
}

/** Состояние в шапке блока: «с 14:02», «работает», «спит». */
function stateOf(turbo: TurboView): string {
    if (turbo.builtin) return tr.dashboard.turboWorks;
    if (turbo.up) return turbo.helper.since ? tr.dashboard.turboUpSince(since(turbo.helper.since, true)) : tr.dashboard.turboUp;
    if (turbo.helper.allowed === false) return tr.dashboard.turboKept;
    return tr.dashboard.turboAsleep;
}

/** Тёмный блок Турбо: имя и где он, на связи ли, его силы — что даёт сейчас светится, остальное — тише. */
export function TurboBody({ turbo, onOpen, title: named, sub: told, state: shown }: { turbo: TurboView; onOpen: () => void; title?: string; sub?: string; state?: string }) {
    const { helper } = turbo;
    const title = named ?? (turbo.builtin ? tr.dashboard.turboBuiltin : tr.dashboard.turboTitle(helper.name));
    const sub = told ?? (turbo.builtin
        ? tr.dashboard.turboBuiltinSub
        : !turbo.up && helper.since
          ? tr.dashboard.turboDownSince(since(helper.since, true))
          : placeTitle(helper.kind));
    const state = shown ?? stateOf(turbo);
    const link = linkOf(turbo);
    const chips = CHIPS.flatMap((id) => turbo.abilities.filter((ability) => ability.id === id));
    return (
        <div class={`turbo-side${turbo.builtin ? ' is-builtin' : ''}`}>
            <span class={`turbo-link ${link.cls}`} aria-hidden="true">
                <span>{link.words}</span>
            </span>
            <button class={`turbo-body${turbo.up ? '' : ' is-off'}${turbo.builtin ? ' is-inner' : ''}`} aria-label={tr.dashboard.turboAria(title, state)} onClick={onOpen}>
                <span class="turbo-head">
                    <span class="turbo-icon" aria-hidden="true">
                        <IconBolt size={18} />
                    </span>
                    <span class="turbo-names">
                        <b>{title}</b>
                        <span>{sub}</span>
                    </span>
                    <span class="turbo-state">
                        <i aria-hidden="true" />
                        {state}
                    </span>
                </span>
                {chips.length > 0 && (
                    <span class="turbo-chips">
                        {chips.map((ability) => {
                            const ChipIcon: Icon = ABILITY_ICONS[ability.id];
                            const taken = turbo.gives.includes(ability.id);
                            const skipped = helper.without.includes(ability.id);
                            const details = ability.details;
                            const what = !ability.ready
                                ? tr.dashboard.turboNotReady
                                : skipped
                                  ? tr.dashboard.turboNotTaken
                                  : ability.id === 'voice'
                                    ? (details?.voice ?? details?.model)
                                    : details?.model;
                            return (
                                <span key={ability.id} class={`turbo-chip${taken || (turbo.up && !skipped && ability.ready) ? '' : ' is-idle'}`}>
                                    <ChipIcon size={15} />
                                    {abilityTitle(ability.id)}
                                    {what && <em>{what}</em>}
                                </span>
                            );
                        })}
                    </span>
                )}
            </button>
        </div>
    );
}

/** Пустое место для Турбо у колонки без него: что он даст и куда идти. */
export function TurboSlot({ onAdd }: { onAdd: () => void }) {
    return (
        <div class="turbo-side">
            <span class="turbo-link is-empty" aria-hidden="true" />
            <div class="turbo-slot">
                <b>{tr.dashboard.slotTitle}</b>
                <span>{tr.dashboard.slotAbout}</span>
                <button class="w-btn" onClick={onAdd}>
                    {tr.dashboard.slotAdd}
                </button>
            </div>
        </div>
    );
}

/** «Если Турбо уснёт: Думает — Claude по ключу, Слышит — Яндекс, Говорит — Алиса». */
export function FallLine({ items, modeFalls, backup }: { items: FallItem[]; modeFalls: boolean; backup?: { title: string; cloud: boolean } }) {
    const t = tr.helpers;
    const who = (item: FallItem): string => {
        switch (item.by) {
            case 'backup':
                return backup ? tr.dashboard.fallBackup(backup.title, backup.cloud) : tr.dashboard.fallNobody;
            case 'alice':
                return tr.dashboard.fallAlice;
            case 'cloud':
                return item.ability === 'hearing' ? t.fallCloudEar : t.fallCloudVoice;
            case 'yandex':
                return item.ability === 'hearing' ? t.fallYandexEar : t.fallAliceVoice;
            case 'text':
                return t.fallText;
            default:
                return tr.dashboard.fallNobody;
        }
    };
    const tone = (item: FallItem): string => (item.by === 'none' || item.by === 'text' ? 'is-none' : '');
    return (
        <p class="turbo-fall">
            <span class="turbo-fall-lead">{tr.dashboard.fallLead}</span>
            {items.map((item) => (
                <span key={item.ability} class={`turbo-fall-go ${tone(item)}`}>
                    <i aria-hidden="true" />
                    {tr.dashboard.fallItem(abilityTitle(item.ability, true), who(item))}
                </span>
            ))}
            {modeFalls && (
                <span class="turbo-fall-go is-none">
                    <i aria-hidden="true" />
                    {t.fallModeBecomes}
                </span>
            )}
        </p>
    );
}

/** Турбо не на связи: полоса над сценой — шлюз работает дальше, в лёгком режиме. */
export function AsleepLine({ turbo, station }: { turbo: TurboView; station: boolean }) {
    const at = turbo.helper.since ? since(turbo.helper.since) : '';
    return (
        <p class="turbo-asleep" role="status">
            {station ? tr.dashboard.asleepStation(at) : tr.dashboard.asleepGateway(at)}
        </p>
    );
}
