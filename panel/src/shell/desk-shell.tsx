// «Главная и док» — основной вид пульта (docs/plans/panel-rebuild.md): тонкая верхняя строка — колонка и её состояние,
// поиск и аватар; главная во весь экран; внизу док — «Главная · Персонаж · [Сказать] · Навыки · Настройки». Разделы
// открываются окнами поверх главной; верхняя строка и док остаются видны. Новый виджет — плиткой на главной. Пока агент
// мастерской делает виджет — в строке его плашка. «Сказать» — микрофон пульта (docs/plans/voice-creation.md): над доком
// полоса голоса; пока открыто поле ключа, кнопка серая и перечёркнута.

import { useEffect, useRef, useState } from 'preact/hooks';

import { IconMic, IconMicOff } from '../icons';
import { onMic, micOn } from '../voice/mic';
import { useMicHeld, useVoiceAction } from '../voice/phrases';
import { VoiceStrip } from '../voice/strip';
import { DuoWaveMark } from '../ui/duowave';
import { AgentPlaque } from './agent-plaque';
import type { NavEntry, ShellProps } from './shell';
import { DeskWindow } from './window';

/** Часы верхней строки: «07:42», раз в полминуты. */
export function useClock(): string {
    const format = () => new Date().toLocaleTimeString('ru-RU', { hour: '2-digit', minute: '2-digit' });
    const [time, setTime] = useState(format);
    useEffect(() => {
        const id = window.setInterval(() => setTime(format()), 30_000);
        return () => window.clearInterval(id);
    }, []);
    return time;
}

function Cell({ entry, active, onNav }: { entry: NavEntry; active: boolean; onNav: (entry: NavEntry) => void }) {
    const { icon: Icon, Badge } = entry;
    // Кнопки дока называют свои фразы: «Навыки», «открой настройки».
    const title = entry.title.toLowerCase();
    useVoiceAction([title, `открой ${title}`], () => onNav(entry), false);
    return (
        <button class="dock-cell" aria-current={active ? 'page' : undefined} onClick={() => onNav(entry)}>
            <span class="tab-icon">
                <Icon size={22} />
                {Badge && <Badge />}
            </span>
            <span class="dock-cell-label">{entry.title}</span>
            {active && <span class="dock-pip" aria-hidden="true" />}
        </button>
    );
}

/** Слушает ли пульт сейчас: «Сказать» с бегущим кольцом. */
function useListening(): boolean {
    const [on, setOn] = useState(micOn);
    useEffect(() => onMic(setOn), []);
    return on;
}

export function DeskShell({ nav, active, onNav, top, actions, talk, notice, desk, window: win }: ShellProps) {
    const dock = useRef<HTMLElement>(null);
    const clock = useClock();
    const listening = useListening();
    const held = useMicHeld();
    // Док шире экрана (телефон, слой разработчика) прокручивается: открытый пункт — в видимой части.
    useEffect(() => {
        dock.current?.querySelector<HTMLElement>('[aria-current="page"]')?.scrollIntoView({ block: 'nearest', inline: 'nearest' });
    }, [active]);
    return (
        <div class="desk-shell">
            <header class="desk-top">
                <DuoWaveMark size={22} title="DuoWave" live />
                <span class="desk-top-line">{top.line}</span>
                {top.narrow && <span class="desk-top-narrow">{top.narrow}</span>}
                <div class="desk-top-end">
                    <AgentPlaque place="top" />
                    {actions}
                    <span class="desk-top-clock" aria-label={`Время: ${clock}`}>
                        {clock}
                    </span>
                </div>
            </header>

            <main class="desk-main" aria-hidden={win ? 'true' : undefined} inert={win ? true : undefined}>
                {notice}
                {desk}
            </main>

            {win && <DeskWindow key={win.id} win={win} />}

            <VoiceStrip />

            <div class="dock-wrap">
                <nav ref={dock} class="dock" aria-label="Док" data-dock="bottom">
                    {nav.left.map((entry) => (
                        <Cell key={entry.key} entry={entry} active={active === entry.key} onNav={onNav} />
                    ))}
                    <button
                        aria-label={held ? 'Микрофон выключен: открыто поле ключа' : listening ? 'Закончить фразу' : 'Сказать'}
                        aria-pressed={listening}
                        class={`talk-btn dock-talk${listening ? ' is-listening' : ''}${held ? ' is-held' : ''}`}
                        disabled={talk.disabled || held}
                        onClick={talk.onTalk}
                    >
                        {held ? <IconMicOff size={22} /> : <IconMic size={22} />}
                    </button>
                    {nav.right.map((entry) => (
                        <Cell key={entry.key} entry={entry} active={active === entry.key} onNav={onNav} />
                    ))}
                </nav>
            </div>
        </div>
    );
}
