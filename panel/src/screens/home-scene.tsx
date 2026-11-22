// Полоса дома под сценой стола (docs/plans/home-and-devices.md, этап 1; холст 17.11, доска «Главная»): колонки, что
// слышат для центра («Колонка «Спальня» · слышит · копия дома»), плашки тех, кто работает вне дома, — Алиса и облака по
// ключам, — узел «Приложения» и легенда цветов: лайм — центр, фиолет — Турбо, янтарь — облака и Яндекс, серый — спит.
// Сам центр и Турбо — на сцене выше (колонка с орбитой и блок Турбо).
//
// На свободном столе со своими виджетами полоса и строка «Если Турбо уснёт» сворачиваются в значок «Поток запросов» под
// колонкой (баг-лист 19.11: «висят мёртвым грузом, хочется расположиться виджетами»): нажали — раскрываются поверх
// стола, ещё раз или мимо — свернулись. Свернуть можно и самому, крестиком в конце полосы; «Не сворачивать» в
// раскрытом — снова как сначала. Выбор помнит пульт. Крестик — в любом окне (замечание 16 после 0.18.1): в узком, где
// сцена стоит в ленте, подписи сворачиваются только им, сами — нет.

import type { ComponentChildren } from 'preact';
import { useEffect, useRef, useState } from 'preact/hooks';

import type { HomeDevice } from '../api';
import { deviceDot, deviceStateWords, useHomeDevices } from '../home-devices';
import { tr } from '../i18n';
import { IconClose, IconCloud, IconEar, IconPackage, IconSpeaker } from '../icons';
import { usePanel } from '../shell/context';
import { openDevice } from '../shell/home-bar';
import { remember } from '../shell/memory';

/** Память пульта: `true` — подписи свёрнуты руками, `false` — «Не сворачивать», нет — сами (свои виджеты на столе). */
export const NOTES_FOLDED = 'desk-notes-folded';

export function HomeStrip({ alice, clouds, onFold }: { alice: boolean; clouds: boolean; onFold?: () => void }) {
    const panel = usePanel();
    const { view } = useHomeDevices();
    const ears: HomeDevice[] = (view?.devices ?? []).filter((device) => device.kind === 'station' && !device.roles.includes('centre'));
    return (
        <div class="home-strip">
            {ears.map((ear) => (
                <button key={ear.id} type="button" class={`home-ear${ear.state === 'online' ? '' : ' is-off'}`} onClick={() => openDevice(panel.go, ear)}>
                    <IconSpeaker size={16} />
                    <span class="home-ear-text">
                        <b>{tr.devices.stationNamed(ear.name)}</b>
                        <span>
                            <IconEar size={12} /> {tr.devices.hears}
                            {ear.copy ? ` · ${tr.devices.copy}` : ''} · {deviceStateWords(ear)}
                        </span>
                    </span>
                    <span aria-hidden="true" class={deviceDot(ear)} />
                </button>
            ))}
            {alice && (
                <span class="home-plaque is-out">
                    <i aria-hidden="true" />
                    {tr.devices.yandex}
                </span>
            )}
            {clouds && (
                <span class="home-plaque is-out">
                    <IconCloud size={14} />
                    {tr.devices.cloud}
                </span>
            )}
            <button type="button" class="home-plaque is-apps" onClick={() => panel.go('skills')}>
                <IconPackage size={14} />
                {tr.devices.apps}
            </button>
            <span class="home-legend" aria-label={tr.devices.homeBarLabel}>
                <span>
                    <i class="is-centre" aria-hidden="true" />
                    {tr.devices.legendCentre}
                </span>
                <span>
                    <i class="is-turbo" aria-hidden="true" />
                    {tr.devices.legendTurbo}
                </span>
                <span>
                    <i class="is-out" aria-hidden="true" />
                    {tr.devices.legendCloud}
                </span>
                <span>
                    <i class="is-asleep" aria-hidden="true" />
                    {tr.devices.legendAsleep}
                </span>
            </span>
            {onFold && (
                <button type="button" class="home-fold" aria-label={tr.devices.notesFold} title={tr.devices.notesFold} onClick={onFold}>
                    <IconClose size={12} />
                </button>
            )}
        </div>
    );
}

/** Свёрнутые подписи дома: значок «Поток запросов» под колонкой; нажали — раскрываются поверх стола. */
export function FoldedNotes({ children }: { children: ComponentChildren }) {
    const t = tr.devices;
    const [open, setOpen] = useState(false);
    const box = useRef<HTMLDivElement>(null);
    useEffect(() => {
        if (!open) return;
        const away = (event: PointerEvent) => {
            if (!box.current?.contains(event.target as Node)) setOpen(false);
        };
        const key = (event: KeyboardEvent) => {
            if (event.key === 'Escape') setOpen(false);
        };
        document.addEventListener('pointerdown', away, true);
        document.addEventListener('keydown', key);
        return () => {
            document.removeEventListener('pointerdown', away, true);
            document.removeEventListener('keydown', key);
        };
    }, [open]);
    return (
        <div ref={box} class={`home-notes${open ? ' is-open' : ''}`}>
            <button type="button" class="home-notes-chip" aria-expanded={open} aria-label={open ? t.notesHide : t.notesShow} onClick={() => setOpen(!open)}>
                <span class="home-notes-dots" aria-hidden="true">
                    <i class="is-centre" />
                    <i class="is-turbo" />
                    <i class="is-out" />
                </span>
                {t.notes}
            </button>
            {open && (
                <div class="home-notes-pop">
                    {children}
                    <button
                        type="button"
                        class="truth-more home-notes-keep"
                        onClick={() => {
                            remember(NOTES_FOLDED, false);
                            setOpen(false);
                        }}
                    >
                        {t.notesKeep}
                    </button>
                </div>
            )}
        </div>
    );
}
