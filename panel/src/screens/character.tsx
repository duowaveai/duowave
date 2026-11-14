// «Персонаж» — одна страница (docs/plans/panel-rebuild.md, этап 2): кто отвечает, его голос, манера и мозг. Глубже — по
// «›»: все персонажи, все голоса и свой голос, модели и провайдеры — прежние экраны вторым слоем. Голос принадлежит
// персонажу, поэтому «Голос» отдельным разделом больше не стоит в доке.

import { useEffect, useRef, useState } from 'preact/hooks';

import { BrainView, CharactersView, ConnectorView, LlmEffort, ModelsView, PersonaInfo, api, can, problemOf } from '../api';
import { IconChevronLeft, IconChevronRight, IconMic, IconSmile, IconSparkle, IconVoice } from '../icons';
import { usePanel, useSectionTab } from '../shell/context';
import { Avatar, PlayButton, markOf } from '../ui/media';
import { BrainTab } from './brain';
import { CharactersTab } from './characters';
import { DashData, THINKING, Thinking, WHERE, nodesOf, thinkingOf, toneClass } from './dash-model';
import { ModelsScreen } from './models';
import { tr } from '../i18n';
import { byMode } from '../words';

export type CharacterTab = 'page' | 'characters' | 'voice' | 'brain';

/** Глубина Claude по ступеням: быстро, с умом, глубоко (как в листе мозга на главной). */
const EFFORT_OF: Record<Thinking, LlmEffort> = { fast: 'low', smart: 'medium', deep: 'high' };

export function CharacterSection() {
    const [tab, setTab] = useSectionTab<CharacterTab>('character', 'page');
    if (tab === 'characters' || tab === 'voice' || tab === 'brain') return <Deeper tab={tab} onBack={() => setTab('page')} />;
    return <CharacterPage onOpen={setTab} />;
}

/** Второй слой: прежний экран целиком и «‹ Персонаж» назад. */
function Deeper({ tab, onBack }: { tab: 'characters' | 'voice' | 'brain'; onBack: () => void }) {
    return (
        <>
            <button class="deeper-back" onClick={onBack}>
                <IconChevronLeft size={16} />
                {tr.characters.characterWord}
            </button>
            {tab === 'characters' && <CharactersTab />}
            {tab === 'voice' && <ModelsScreen tab="voice" onTab={() => undefined} bare />}
            {tab === 'brain' && <BrainTab />}
        </>
    );
}

/** Строка страницы: имя, что сейчас, и «›» вглубь. */
function Row({
    icon: RowIcon,
    title,
    value,
    sub,
    tone,
    onOpen,
    children,
}: {
    icon: typeof IconVoice;
    title: string;
    value?: string;
    sub?: string;
    tone?: string;
    onOpen?: () => void;
    children?: preact.ComponentChildren;
}) {
    return (
        <div class="char-row">
            <button class="char-row-head" onClick={onOpen} disabled={!onOpen}>
                <span class="set-row-icon" aria-hidden="true">
                    <RowIcon size={18} />
                </span>
                <span class="char-row-text">
                    <b>{title}</b>
                    {sub && <span class="w-cap">{sub}</span>}
                </span>
                {value && <span class={`char-row-value${tone ? ` ${tone}` : ''}`}>{value}</span>}
                {onOpen && <IconChevronRight size={16} />}
            </button>
            {children}
        </div>
    );
}

function CharacterPage({ onOpen }: { onOpen: (tab: CharacterTab) => void }) {
    const panel = usePanel();
    const [view, setView] = useState<CharactersView | null>(null);
    const [models, setModels] = useState<ModelsView | null>(null);
    const [brainView, setBrainView] = useState<BrainView | null>(null);
    const [persona, setPersona] = useState<PersonaInfo | null>(null);
    const [connectors, setConnectors] = useState<ConnectorView[] | null>(null);
    const [busy, setBusy] = useState<string | null>(null);
    const [note, setNote] = useState<{ ok: boolean; text: string } | null>(null);
    const alive = useRef(true);
    useEffect(() => () => void (alive.current = false), []);
    const read = (): void => {
        if (can('station.view')) {
            void api.characters().then((fresh) => alive.current && setView(fresh)).catch(() => undefined);
            void api.models().then((fresh) => alive.current && setModels(fresh)).catch(() => undefined);
            void api.persona().then((fresh) => alive.current && setPersona(fresh)).catch(() => undefined);
        }
        if (can('connectors.view')) {
            void api.brain().then((fresh) => alive.current && setBrainView(fresh)).catch(() => undefined);
            void api
                .connectors()
                .then((fresh) => alive.current && setConnectors(fresh.connectors))
                .catch(() => undefined);
        }
    };
    useEffect(read, [panel.version]);

    const health = panel.health;
    const mode = health?.panelMode ?? (health?.mode === 'relay' ? 'relay' : 'gateway');
    const data: DashData = { mode, routes: health?.skills?.routes, brain: brainView?.status ?? health?.brain, brainView, models, persona, connectors };
    const nodes = nodesOf(data);
    const active = view?.active ?? null;
    const character = active?.id ? view?.characters.find((entry) => entry.id === active.id) : undefined;
    const name = character?.title ?? (active ? tr.characters.ownCharacter : tr.characters.characterWord);
    const relay = mode === 'relay';

    const act = async (what: string, job: () => Promise<string>): Promise<void> => {
        setBusy(what);
        setNote(null);
        try {
            setNote({ ok: true, text: await job() });
            read();
        } catch (e) {
            setNote({ ok: false, text: problemOf(e) });
        } finally {
            setBusy(null);
        }
    };

    // Манера: тоны персонажа — сразу, со следующего вопроса; голос при этом не меняется.
    const tones = character?.tones ?? [];
    const toneAt = (id: string, fallback: number): number => active?.tones?.[id] ?? fallback;
    const setTone = (id: string, at: number) =>
        character &&
        void act(`tone:${id}`, async () => {
            const next = Object.fromEntries(tones.map((tone) => [tone.id, tone.id === id ? at : toneAt(tone.id, tone.default)]));
            await api.chooseCharacter(character.id, { tones: next, voice: false });
            return tr.characters.mannerApplied;
        });

    // Мозг: глубина выбирается у Claude из пульта; у других моделей — только видно, какая модель.
    const brain = data.brain;
    const claude = brain?.provider === 'anthropic' && brain.source === 'panel' && nodes.brain.tone !== 'yandex' && nodes.brain.tone !== 'off';
    const level: Thinking | null = brain && claude ? thinkingOf(brain) : null;
    const setLevel = (next: Thinking) =>
        void act(`think:${next}`, async () => {
            await api.chooseBrain({ effort: EFFORT_OF[next] });
            return tr.characters.appliedNext;
        });

    return (
        <div class="char-page">
            <section class="card glass char-hero" aria-label={tr.characters.whoAnswers}>
                <Avatar src={character?.avatar} title={name} color={character?.color} size="lg" mark={markOf(character)} />
                <div class="char-hero-text">
                    <span class="w-cap">{tr.characters.nowAnswering}</span>
                    <h2 class="char-hero-name">{relay ? tr.characters.alice : name}</h2>
                    <p class="t-meta m-0">
                        {relay
                            ? tr.characters.relayAnswers
                            : (character?.description ?? tr.characters.ownTextDescription)}
                    </p>
                </div>
                <div class="char-hero-actions">
                    {character && <PlayButton id={character.id} url={character.samples[0]?.url} label={character.title} className="char-play is-inline" />}
                    <button class="w-btn" onClick={() => onOpen('characters')}>
                        {tr.characters.chooseAnother}
                    </button>
                </div>
            </section>

            <section class="card glass char-rows" aria-label={tr.characters.rowsLabel}>
                <Row
                    icon={IconVoice}
                    title={tr.characters.voice}
                    value={nodes.voice.value}
                    sub={nodes.voice.tone !== 'off' ? WHERE[nodes.voice.tone] : nodes.voice.detail}
                    tone={toneClass(nodes.voice.tone)}
                    onOpen={() => onOpen('voice')}
                />
                <Row icon={IconSmile} title={tr.characters.manner} sub={tones.length ? tr.characters.mannerSubTones : tr.characters.mannerSubOwn} onOpen={() => panel.go('character', 'characters', active?.id ?? 'own')}>
                    {tones.length > 0 && !relay && (
                        <div class="char-tones">
                            {tones.map((tone) => (
                                <div key={tone.id} class="char-tone">
                                    <span class="w-cap">{tone.title}</span>
                                    <div
                                        class="glass grid gap-4 p-4 r-pill"
                                        style={{ gridTemplateColumns: `repeat(${tone.options.length}, minmax(0, 1fr))` }}
                                        role="radiogroup"
                                        aria-label={tone.title}
                                    >
                                        {tone.options.map((option, at) => (
                                            <button
                                                key={option.label}
                                                role="radio"
                                                aria-checked={toneAt(tone.id, tone.default) === at}
                                                class="seg-btn"
                                                disabled={!can('persona.edit') || busy !== null}
                                                onClick={() => setTone(tone.id, at)}
                                            >
                                                {option.label}
                                            </button>
                                        ))}
                                    </div>
                                </div>
                            ))}
                        </div>
                    )}
                </Row>
                <Row
                    icon={IconSparkle}
                    title={tr.characters.brain}
                    value={nodes.brain.value}
                    sub={nodes.brain.tone !== 'off' ? WHERE[nodes.brain.tone] : nodes.brain.detail}
                    tone={toneClass(nodes.brain.tone)}
                    onOpen={() => onOpen('brain')}
                >
                    {claude && (
                        <div class="char-tone">
                            <span class="w-cap">{tr.characters.thinkLonger}</span>
                            <div class="glass grid grid-cols-3 gap-4 p-4 r-pill" role="radiogroup" aria-label={tr.characters.howToThink}>
                                {(Object.keys(THINKING) as Thinking[]).map((id) => (
                                    <button
                                        key={id}
                                        role="radio"
                                        aria-checked={level === id}
                                        class="seg-btn"
                                        disabled={!can('models.manage') || busy !== null}
                                        onClick={() => setLevel(id)}
                                    >
                                        {THINKING[id].name}
                                    </button>
                                ))}
                            </div>
                        </div>
                    )}
                </Row>
                <Row icon={IconMic} title={tr.characters.ownVoice} sub={byMode(tr.characters.ownVoiceSubStation, tr.characters.ownVoiceSubDevice)} onOpen={() => onOpen('voice')} />
            </section>
            {note && (
                <p role={note.ok ? 'status' : 'alert'} class={`t-meta m-0 ${note.ok ? 'c-text' : 'c-dot-offline'}`}>
                    {note.text}
                </p>
            )}
        </div>
    );
}
