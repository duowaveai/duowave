// «Где я» (решение владельца 16.11, доски DuoDesk и HelperDesk): вкладки устройств дома в верхней строке — «Колонка
// «Кухня» · шлюз | Этот компьютер · Турбо», нажатая — чей это пульт, рядом строка «Вы в пульте колонки». Переход на
// другую вкладку — одно нажатие (shell/role.ts, whereAmI). На телефоне — значок и короткое имя, без строки.

import { FunctionComponent } from 'preact';

import { tr } from '../i18n';
import { IconChevronDown, IconLaptop, IconServer, IconSpeaker } from '../icons';
import { flushPanelMemory } from './memory';
import type { DeviceTab, WhereAmI } from './role';

type Icon = FunctionComponent<{ size?: number }>;

const ICONS: Record<DeviceTab['kind'], Icon> = { station: IconSpeaker, computer: IconLaptop, server: IconServer };

/** Имя вкладки: «Колонка «Кухня»», «Этот компьютер», имя Турбо. */
export function tabName(tab: DeviceTab): string {
    if (tab.kind === 'station') return tab.name ? tr.core.tabStation(tab.name) : tr.core.tabStationNoName;
    if (tab.key === 'computer') return tr.core.tabThisComputer;
    return tab.name ?? tr.core.tabThisComputer;
}

/** Короткое имя для телефона: «Кухня», «Компьютер». */
function shortName(tab: DeviceTab): string {
    if (tab.kind === 'station') return tab.name ?? tr.core.tabStationNoName;
    return tab.key === 'computer' ? tr.core.tabThisComputer : (tab.name ?? '');
}

function tagWord(tag: NonNullable<DeviceTab['tag']>): string {
    return { gateway: tr.core.tagGateway, turbo: tr.core.tagTurbo, gatewayTurbo: tr.core.tagGatewayTurbo, asleep: tr.core.tagAsleep }[tag];
}

/** Перейти в другой пульт: сначала память пульта (ещё не отправленное), потом адрес. */
function go(href: string): void {
    void flushPanelMemory().then(() => location.assign(href));
}

export function DeviceTabs({
    where,
    short = false,
    dot,
    picking,
    onPick,
}: {
    where: WhereAmI;
    /** Телефон: значок и короткое имя. */
    short?: boolean;
    /** Точка связи у нажатой вкладки (телефон: строки состояния нет). */
    dot?: string;
    picking?: boolean;
    onPick?: () => void;
}) {
    return (
        <span class={`dev-tabs${short ? ' is-short' : ''}`} role="group" aria-label={tr.core.tabsLabel}>
            {where.tabs.map((tab) => {
                const TabIcon = ICONS[tab.kind];
                const name = short ? shortName(tab) : tabName(tab);
                const body = (
                    <>
                        {short && tab.on && dot ? <span aria-hidden="true" class={dot} /> : <TabIcon size={short ? 15 : 14} />}
                        <span class="dev-tab-name">{name}</span>
                        {!short && tab.tag && <span class={`dev-tab-tag${tab.tag === 'turbo' || tab.tag === 'gatewayTurbo' ? ' is-turbo' : ''}`}>{tagWord(tab.tag)}</span>}
                        {tab.on && tab.pick && <IconChevronDown />}
                    </>
                );
                const cls = `dev-tab${tab.on ? ' is-on' : ''}${tab.off ? ' is-off' : ''}`;
                if (tab.on && tab.pick && onPick)
                    return (
                        <button key={tab.key} class={cls} aria-current="page" aria-haspopup="dialog" aria-expanded={picking} onClick={onPick}>
                            {body}
                        </button>
                    );
                if (tab.on)
                    return (
                        <span key={tab.key} class={cls} aria-current="page">
                            {body}
                        </span>
                    );
                if (tab.href) {
                    const href = tab.href;
                    return (
                        <a
                            key={tab.key}
                            class={cls}
                            href={href}
                            aria-label={short ? tr.core.tabOpen(tabName(tab)) : undefined}
                            onClick={(event) => {
                                event.preventDefault();
                                go(href);
                            }}
                        >
                            {body}
                        </a>
                    );
                }
                return (
                    <span key={tab.key} class={cls}>
                        {body}
                    </span>
                );
            })}
        </span>
    );
}

/** «Вы в пульте колонки», а в пульте Турбо — «… компьютера — он Турбо и сам не отвечает». */
export function HereLine({ where, turbo }: { where: WhereAmI; turbo?: boolean }) {
    if (!where.here) return null;
    return (
        <span class="dev-here">
            {tr.core.hereLead} <b>{where.here === 'station' ? tr.core.hereStation : tr.core.hereComputer}</b>
            {turbo && ` ${tr.core.hereTurbo}`}
        </span>
    );
}
