// Стол — главный экран пульта (docs/plans/desk.md, макеты docs/design/roles): колонка в середине — сцена с орбитой,
// Слух · Мозг · Голос и карточкой ответа, — вокруг виджеты: встроенные и из плагинов. На широком экране виджеты стоят
// по сторонам от колонки; на телефоне — лента: колонка посередине, виджеты выше и ниже, маленькие по два в ряд; в
// виде с боковой панелью — столбцом под колонкой. Что стоит на столе — у каждой роли своё, и хранит это шлюз, а не
// браузер (shell/widgets.ts). Замок закрепляет только свободный стол: у ленты и столбца поля нет.

import { useEffect, useState } from 'preact/hooks';

import { usePanel } from '../shell/context';
import { useRecalled } from '../shell/memory';
import { useTheme } from '../shell/theme';
import { DeskWidget, isFresh, orderedOnDesk, repairedLayout, useDeskLayout, useWidgets } from '../shell/widgets';
import { landingsOf } from '../shell/desk-repair';
import { WidgetFrame, WidgetMenu } from '../ui/widget';
import { settingsPlace } from '../builtin';
import { ConnectFirstCard } from './connect';
import { FirstRun } from './first-run';
import { MicFirstCard } from './voice-input';
import { LocalAsrSetup, localAsrNeedsCard, useLocalAsr } from './local-asr';
import { Dashboard } from './dashboard';
import { DeskCanvas } from './desk-canvas';
import { TurboDesk } from './turbo-desk';
import { NOTES_FOLDED } from './home-scene';
import { VIA } from '../via';
import { useWhoOpen } from './desk-widgets';
import { AddTile, DraftTile } from './draft-tile';
import { FolderOffers, useFolderOffers } from './folder-offers';
import { DeskDrafts, useDeskDrafts } from '../voice/drafts';

export { AddTile };
import { RememberedLine } from './recall';
import { DeskAsk } from './desk-ask';
import { Shortcuts } from './shortcuts';

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

/** Стол пульта Турбо: компьютер только помогает — вместо орбиты его Турбо и шлюзы, которым он помогает. */
const turboPanel = (role: string): boolean => role === 'helper' && !VIA;

/**
 * Стол пульта Турбо (его тело — в сцене, справа от «Здесь не разговаривают»): сцене на свободном столе нужно места на
 * него справа. Столько точек ему. Карточка Турбо рядом со шлюзом места не берёт: она висит поверх стола (баг-лист
 * 19.11, dashboard.tsx).
 */
const TURBO_W = 360;

/** Сцена колонки — середина стола. `room` — её высота, без неё — до низа экрана; `notesFolded` — подписи свёрнуты. */
function Scene({ room, notesFolded }: { room?: number; notesFolded?: boolean }) {
    const panel = usePanel();
    const [whoOpen, toggleWho] = useWhoOpen();
    if (turboPanel(panel.role.role))
        return (
            <div class="desk-scene">
                <TurboDesk />
            </div>
        );
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
                onOpenHome={() => panel.go('settings', 'helpers')}
                onOpenTurbo={(id) => panel.go('settings', 'devices', `turbo:${id}`, { view: 'computer' })}
                notesFolded={notesFolded}
            />
            {/* Персонаж только что запомнил что-то голосом или мимоходом: видно сразу, с «Отменить». */}
            {/* Строки под сценой — своим блоком с отступом снизу: широкий виджет под сценой (витрина) заходит на пустой низ
                кольца, но не на них. Нет строк — блока нет. */}
            {panel.role.role !== 'helper' && (
                <div class="desk-scene-lines">
                    <RememberedLine />
                    {/* Второй раз спросили то, на что отвечает приложение со своей карточкой, — «Поставить на стол?». */}
                    <DeskAsk />
                </div>
            )}
        </div>
    );
}

/**
 * Место «Новый виджет» (docs/plans/voice-creation.md): черновик, над которым агент работает или который ждёт на
 * примерке, стоит плиткой здесь — вещь рождается там, где будет стоять, — а «Новый виджет» сдвигается ниже.
 */
function Making({ drafts, onAdd }: { drafts: DeskDrafts; onAdd: () => void }) {
    const { tile, more } = drafts;
    return (
        <>
            {tile && <DraftTile key={tile.id} draft={tile} more={more} />}
            <AddTile onAdd={onAdd} small={!!tile} />
        </>
    );
}

/** Плитка-черновик первой в ленте телефона. */
function FeedDraft({ drafts }: { drafts: DeskDrafts }) {
    const { tile, more } = drafts;
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
    // Флешку воткнули или папку бросили на окно: плитка «Посмотреть?» — над столом, в любой раскладке. Страну дома пульт
    // не спрашивает (владелец, 16.11): её спрашивает в своём окне приложение, которому она нужна («VPN»).
    const offers = useFolderOffers();
    const banner =
        panel.banner || setup || offers.length > 0 ? (
            <>
                {panel.banner}
                {setup}
                <FolderOffers offers={offers} />
            </>
        ) : null;
    // Плитка, положенная под ключом, которого стол не рисует (п. 47), — на настоящую плитку пакета, и это сохраняется.
    const repaired = repairedLayout(layout, widgets);
    useEffect(() => {
        if (repaired) actions.repair(widgets);
    }, [repaired !== null]);
    // Виджет, поставленный голосом мимо «Примерки» (шлюз сказал «На столе.»), садится на стол, как только стол его рисует.
    const drafts = useDeskDrafts();
    const landings = landingsOf(drafts.installed, repaired ?? layout, widgets);
    useEffect(() => {
        for (const key of landings) actions.land(key);
    }, [landings.join()]);
    const ordered = orderedOnDesk(widgets, repaired ?? layout);
    // Место справа от колонки — только столу пульта Турбо: карточка Турбо у шлюза висит поверх стола.
    const sceneExtra = turboPanel(role) ? TURBO_W : 0;
    // Подписи под колонкой сворачиваются, когда на столе свои виджеты (поставили сами или убрали стартовые): место —
    // виджетам. Свернуть самому и «Не сворачивать» — память пульта.
    const notesChoice = useRecalled<boolean>(NOTES_FOLDED);
    const notesFolded = notesChoice ?? (layout.added.length > 0 || layout.hidden.length > 0);
    // Без колонки столу нечего показать: режимы и орбита были бы о колонке, которой нет. Виджеты «под сценой» —
    // приложения этого компьютера («Лента») — стоят и здесь, под первыми шагами: им колонка не нужна.
    if (panel.noStation)
        return (
            <div class="desk-solo">
                {banner}
                <Shortcuts layout={layout} actions={actions} place="row" />
                <FirstRun />
                <ConnectFirstCard onConnect={panel.openConnect} />
                <MicFirstCard />
                {ordered
                    .filter((widget) => widget.under)
                    .map((widget) => (
                        <WidgetOnDesk key={widget.key} widget={widget} menu={{ hide: () => actions.hide(widget.key) }} fresh={isFresh(layout, widget.key)} />
                    ))}
            </div>
        );
    // Виджеты «под сценой» (`under`) — сразу под колонкой или ДиВи: в ленте телефона первыми после сцены, в столбце —
    // первыми; на свободном столе их ставит DeskCanvas.
    const shown = [...ordered.filter((widget) => widget.under), ...ordered.filter((widget) => !widget.under)];
    const unders = shown.filter((widget) => widget.under);
    const others = shown.filter((widget) => !widget.under);
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
                <Shortcuts layout={layout} actions={actions} place="row" />
                <Scene />
                <div class="desk-column">{shown.map((widget) => tile(widget, true))}</div>
            </>
        );

    // Телефон и узкое окно: лента — колонка посередине, по виджету-два выше, остальные ниже. Плитка-черновик — первой в
    // ленте (решение 9), над колонкой.
    if (!wide) {
        const above = others.slice(0, Math.min(2, Math.floor(others.length / 3)));
        const below = [...unders, ...others.slice(above.length)];
        return (
            <div class="desk-feed">
                {banner}
                <Shortcuts layout={layout} actions={actions} place="row" />
                <FeedDraft drafts={drafts} />
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
        <>
            {/* Ярлыки — у левого края, на месте, когда стол двигают и масштабируют. */}
            <Shortcuts layout={layout} actions={actions} place="column" />
            <DeskCanvas
            widgets={shown}
            sceneExtra={sceneExtra}
            layout={layout}
            actions={actions}
            scene={<Scene room={460} notesFolded={notesFolded} />}
            addTile={<Making drafts={drafts} onAdd={panel.openAdd} />}
            banner={banner}
            render={(widget, menu, grip) => <WidgetOnDesk widget={widget} menu={menu} grip={grip} fresh={isFresh(layout, widget.key)} />}
            />
        </>
    );
}
