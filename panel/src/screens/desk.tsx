// Стол — главный экран пульта (docs/plans/desk.md, макеты docs/design/roles): колонка в середине — сцена с орбитой,
// Слух · Мозг · Голос и карточкой ответа, — вокруг виджеты: встроенные и из плагинов. На широком экране виджеты стоят
// по сторонам от колонки; на телефоне — лента: колонка посередине, виджеты выше и ниже, маленькие по два в ряд; в
// виде с боковой панелью — столбцом под колонкой. Что стоит на столе — у каждой роли своё, и хранит это шлюз, а не
// браузер (shell/widgets.ts). Замок закрепляет только свободный стол: у ленты и столбца поля нет.

import { useEffect, useState } from 'preact/hooks';

import type { PlacedStation } from '../api';
import { IconLaptop } from '../icons';
import { usePanel } from '../shell/context';
import { useTheme } from '../shell/theme';
import { DeskWidget, isFresh, orderedOnDesk, useDeskLayout, useWidgets } from '../shell/widgets';
import { WidgetFrame, WidgetMenu } from '../ui/widget';
import { settingsPlace } from '../builtin';
import { ConnectFirstCard } from './connect';
import { FirstRun } from './first-run';
import { MicFirstCard } from './voice-input';
import { LocalAsrSetup, localAsrNeedsCard, useLocalAsr } from './local-asr';
import { Dashboard } from './dashboard';
import { DeskCanvas } from './desk-canvas';
import { useWhoOpen } from './desk-widgets';
import { AddTile, DraftTile } from './draft-tile';
import { FolderOffers, useFolderOffers } from './folder-offers';
import { useDeskDrafts } from '../voice/drafts';

export { AddTile };
import { RememberedLine } from './recall';
import { tr } from '../i18n';

/** С этой ширины виджеты стоят по сторонам от колонки; уже — лентой. */
const WIDE = '(min-width: 1100px)';

export function useMedia(query: string): boolean {
    const [matches, setMatches] = useState(() => matchMedia(query).matches);
    useEffect(() => {
        const list = matchMedia(query);
        const update = () => setMatches(list.matches);
        update();
        list.addEventListener('change', update);
        return () => list.removeEventListener('change', update);
    }, [query]);
    return matches;
}

/** Сцена колонки — середина стола. `room` — её высота, без неё — до низа экрана. */
function Scene({ room }: { room?: number }) {
    const panel = usePanel();
    const [whoOpen, toggleWho] = useWhoOpen();
    return (
        <div class="desk-scene">
            <Dashboard
                health={panel.health}
                session={panel.session}
                drawing={panel.drawing}
                online={panel.online}
                trip={panel.trip}
                skew={panel.feed.skew}
                version={panel.version}
                onOpenSettings={(place) => panel.go(...settingsPlace(place))}
                onOpenHouse={(connector) => panel.openHouse(connector)}
                whoOpen={whoOpen}
                onToggleWho={toggleWho}
                room={room}
            />
            {/* Персонаж только что запомнил что-то голосом или мимоходом: видно сразу, с «Отменить». */}
            {panel.role.role !== 'helper' && <RememberedLine />}
            {panel.role.role === 'helper' && (
                // Макет SpaceHelper: под колонкой — этот компьютер, её помощник.
                <div class="desk-helper-pc">
                    <span class="desk-helper-pc-icon" aria-hidden="true">
                        <IconLaptop size={18} />
                    </span>
                    <span class="flex flex-col">
                        <b>{tr.desk.helperPcTitle}</b>
                        <span class="w-cap">{tr.desk.helperPcLine(placedName(panel.role.station) ?? panel.current?.name ?? panel.drawing.title)}</span>
                    </span>
                </div>
            )}
        </div>
    );
}

/**
 * Имя колонки помощника из записи компьютера о её размещении (/health → placement.stations[].name): выбранная в
 * списке колонок может быть другой, а пока список не прочитан, пульт знает только «Колонка». Шлюз постарше имени не
 * присылает.
 */
function placedName(station: PlacedStation | undefined): string | undefined {
    return (station as (PlacedStation & { name?: string }) | undefined)?.name || undefined;
}

/**
 * Место «Новый виджет» (docs/plans/voice-creation.md): черновик, над которым агент работает или который ждёт на
 * примерке, стоит плиткой здесь — вещь рождается там, где будет стоять, — а «Новый виджет» сдвигается ниже.
 */
function Making({ onAdd }: { onAdd: () => void }) {
    const { tile, more } = useDeskDrafts();
    return (
        <>
            {tile && <DraftTile key={tile.id} draft={tile} more={more} />}
            <AddTile onAdd={onAdd} small={!!tile} />
        </>
    );
}

/** Плитка-черновик первой в ленте телефона. */
function FeedDraft() {
    const { tile, more } = useDeskDrafts();
    return tile ? <DraftTile key={tile.id} draft={tile} more={more} /> : null;
}

export function WidgetOnDesk({ widget, menu, grip, fresh }: { widget: DeskWidget; menu: WidgetMenu; grip?: (event: PointerEvent) => void; fresh?: boolean }) {
    const { Component } = widget;
    return (
        <WidgetFrame widget={widget} menu={menu} grip={grip} fresh={fresh}>
            <Component />
        </WidgetFrame>
    );
}

export function DeskScreen() {
    const panel = usePanel();
    const { view } = useTheme();
    const wide = useMedia(WIDE);
    const role = panel.role.role;
    const widgets = useWidgets(role, panel.health);
    const [layout, actions] = useDeskLayout(role);
    // Слух этого компьютера, пока он готовится или упал: над всем остальным, в любой раскладке.
    const [localAsr, setLocalAsr] = useLocalAsr();
    const setup = localAsrNeedsCard(localAsr) ? <LocalAsrSetup local={localAsr} onChange={setLocalAsr} /> : null;
    // Флешку воткнули или папку бросили на окно: плитка «Посмотреть?» — над столом, в любой раскладке.
    const offers = useFolderOffers();
    const banner =
        panel.banner || setup || offers.length > 0 ? (
            <>
                {panel.banner}
                {setup}
                <FolderOffers offers={offers} />
            </>
        ) : null;
    // Без колонки столу нечего показать: режимы и орбита были бы о колонке, которой нет.
    if (panel.noStation)
        return (
            <div class="desk-solo">
                {banner}
                <FirstRun />
                <ConnectFirstCard onConnect={panel.openConnect} />
                <MicFirstCard />
            </div>
        );
    const shown = orderedOnDesk(widgets, layout);
    const keys = shown.map((widget) => widget.key);
    const menuOf = (widget: DeskWidget, feed: boolean): WidgetMenu => ({
        hide: () => actions.hide(widget.key),
        earlier: feed && keys.indexOf(widget.key) > 0 ? () => actions.move(widget.key, -1, keys) : undefined,
        later: feed && keys.indexOf(widget.key) < keys.length - 1 ? () => actions.move(widget.key, 1, keys) : undefined,
    });
    const tile = (widget: DeskWidget, feed: boolean) => (
        <WidgetOnDesk key={widget.key} widget={widget} menu={menuOf(widget, feed)} fresh={isFresh(layout, widget.key)} />
    );

    // Боковая панель: колонка, под ней виджеты столбцом.
    if (view === 'sidebar')
        return (
            <>
                {banner}
                <Scene />
                <div class="desk-column">{shown.map((widget) => tile(widget, true))}</div>
            </>
        );

    // Телефон и узкое окно: лента — колонка посередине, по виджету-два выше, остальные ниже. Плитка-черновик — первой в
    // ленте (решение 9), над колонкой.
    if (!wide) {
        const above = shown.slice(0, Math.min(2, Math.floor(shown.length / 3)));
        const below = shown.slice(above.length);
        return (
            <div class="desk-feed">
                {banner}
                <FeedDraft />
                {above.length > 0 && <div class="desk-feed-grid">{above.map((widget) => tile(widget, true))}</div>}
                <Scene room={420} />
                <div class="desk-feed-grid">
                    {below.map((widget) => tile(widget, true))}
                    <AddTile onAdd={panel.openAdd} />
                </div>
            </div>
        );
    }

    // Широкий экран: свободный стол — колонка в середине поля, виджеты там, где их поставили (этап 3).
    return (
        <DeskCanvas
            widgets={shown}
            layout={layout}
            actions={actions}
            scene={<Scene room={460} />}
            addTile={<Making onAdd={panel.openAdd} />}
            banner={banner}
            render={(widget, menu, grip) => <WidgetOnDesk widget={widget} menu={menu} grip={grip} fresh={isFresh(layout, widget.key)} />}
        />
    );
}
