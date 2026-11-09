// Служебная страница-превью всех рисунков станций: собирается вручную (esbuild), в панель не входит.
// Назначение — глазами проверить новый силуэт и анимацию до того, как их увидит пульт:
//   node_modules/.bin/esbuild src/preview.tsx --bundle --jsx=automatic --jsx-import-source=preact --outfile=<папка>/preview.js
// и открыть рядом html с <script src="preview.js">.

import { render } from 'preact';

import { MODELS } from './stations/models';
import './stations/stations.css';
import type { StationStage } from './stations/types';

interface Card {
    id: string;
    code: string;
    colors: string[];
    clock: boolean[];
}

const CARDS: Card[] = [
    { id: 'lite2', code: 'plum', colors: ['azure', 'lilac', 'pink', 'green', 'coral', 'graphite'], clock: [true, false] },
    { id: 'lite1', code: 'yandexmicro', colors: ['lilac', 'teal', 'red', 'yellow'], clock: [false] },
    { id: 'mini1', code: 'yandexmini', colors: ['graphite', 'white', 'azure', 'red'], clock: [false] },
    { id: 'mini2', code: 'yandexmini_2', colors: ['graphite', 'lilac', 'red', 'azure'], clock: [true, false] },
    { id: 'mini3', code: 'bergamot', colors: ['graphite', 'lilac', 'white', 'green'], clock: [true, false] },
    { id: 'mini3pro', code: 'pickle', colors: ['graphite', 'green', 'red'], clock: [true] },
    { id: 'station', code: 'yandexstation', colors: ['graphite'], clock: [false] },
    { id: 'max', code: 'yandexstation_2', colors: ['graphite', 'sand', 'red'], clock: [true] },
    { id: 'station2', code: 'yandexmidi', colors: ['graphite', 'azure', 'copper', 'sand'], clock: [true] },
    { id: 'midi', code: 'cucumber', colors: ['teal', 'graphite', 'orange'], clock: [true, false] },
    { id: 'duomax', code: 'chiron', colors: ['graphite', 'green'], clock: [true] },
    { id: 'station3', code: 'orion', colors: ['graphite'], clock: [true] },
    { id: 'street', code: 'mango', colors: ['orange', 'graphite', 'yellow'], clock: [false] },
    { id: 'bork', code: 'bork', colors: ['white'], clock: [true] },
];

const STAGES: StationStage[] = ['idle', 'listen', 'think', 'speak'];
const RING = '#B6F36A';

function App() {
    const params = new URLSearchParams(location.search);
    const only = params.get('only');
    const big = Number(params.get('w') ?? 130);
    const stages = params.get('stage')?.split(',') ?? STAGES;
    return (
        <main style={`display: grid; grid-template-columns: repeat(auto-fill, minmax(${big * 4 + 50}px, 1fr)); gap: 16px; padding: 24px; background: #10151c; color: #e8edf4;`}>
            {CARDS.filter((card) => !only || only.split(',').includes(card.id)).map((card) => {
                const { Art, Glyph, title } = MODELS[card.id];
                return (
                    <section key={card.id} style="background: #1a2230; border-radius: 14px; padding: 12px;">
                        <h2 style="margin: 0 0 8px; font: 600 15px/1.3 system-ui; display: flex; gap: 8px; align-items: center;">
                            <Glyph color={card.colors[0]} /> <Glyph color={card.colors[0]} size={18} /> {title} · {card.code}
                        </h2>
                        <div style="display: flex; gap: 6px;">
                            {(stages as StationStage[]).map((stage) => (
                                <div key={stage} style={{ width: `${big}px` }}>
                                    <Art skin={card.colors[0]} clock={card.clock[0]} sticker={card.id === 'lite2'} ring={RING} stage={stage} />
                                    <div style="font: 11px/1.4 system-ui; opacity: 0.7; text-align: center;">{stage}</div>
                                </div>
                            ))}
                        </div>
                        <div style="display: flex; gap: 6px; flex-wrap: wrap; align-items: end; margin-top: 6px;">
                            {card.clock.map((clock) =>
                                card.colors.map((color) => (
                                    <div key={`${clock}-${color}`} style="width: 64px;">
                                        <Art skin={color} clock={clock} ring={RING} stage="idle" />
                                        <div style="font: 10px/1.3 system-ui; opacity: 0.6; text-align: center;">
                                            {color}
                                            {clock ? ' ⏰' : ''}
                                        </div>
                                    </div>
                                )),
                            )}
                            <div style="width: 64px;">
                                <Art skin={card.colors[0]} clock={card.clock[0]} ring={RING} stage="idle" offline />
                                <div style="font: 10px/1.3 system-ui; opacity: 0.6; text-align: center;">offline</div>
                            </div>
                        </div>
                    </section>
                );
            })}
        </main>
    );
}

render(<App />, document.body);
