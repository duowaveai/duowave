// «Компьютер-помощник» (docs/plans/panel-rebuild.md, этап 4). Когда компьютер помогает колонке, окно приложения —
// пульт колонки: всё в нём идёт к ней через проход компьютера. Своё у компьютера — эта страница: помогать ли колонке,
// не засыпать, копии и страховка, обновления и модели самого компьютера, его мастерская. Она говорит с шлюзом
// компьютера напрямую, поэтому в пульте колонки стоит фреймом без рамки (`?own=helper`, `?own=workshop`): своими
// запросами, своим входом, тем же оформлением. Колонка не отвечает — страница всё равно работает.

import { useEffect, useState } from 'preact/hooks';

import { can } from '../api';
import { tr } from '../i18n';
import { HelperWidget, Watch } from './desk-widgets';
import { Updates } from './house';
import { ModelsScreen } from './models';
import { PlacementCard } from './placement';
import { WorkshopScreen } from './workshop';

/** Страница компьютера внутри фрейма: «helper» — «Компьютер-помощник», «workshop» — мастерская. */
export function OwnPage({ page }: { page: string }) {
    // Фрейм сливается со страницей: прозрачный фон, без полей окна.
    useEffect(() => {
        document.documentElement.classList.add('own-page');
        return () => document.documentElement.classList.remove('own-page');
    }, []);
    return <div class="own-page-body">{page === 'workshop' ? <WorkshopScreen /> : <HelperComputer />}</div>;
}

function Card({ id, title, children }: { id: string; title: string; children: preact.ComponentChildren }) {
    return (
        <section class="card glass px-16 pt-4 pb-14 flex flex-col gap-10" aria-labelledby={`pc-${id}`}>
            <div class="section-head">
                <h2 id={`pc-${id}`} class="t-title-xs m-0">
                    {title}
                </h2>
            </div>
            {children}
        </section>
    );
}

/** Своё у компьютера: помощь колонке, присмотр, обновления, а глубже — его модели. */
function HelperComputer() {
    const [models, setModels] = useState<'voice' | 'recognition' | null>(null);
    // «Открыть» у присмотра: всё о размещении — здесь же, глазами компьютера (копии, перенос, что случилось).
    const [placement, setPlacement] = useState(false);
    return (
        <>
            <Card id="help" title={tr.helperPc.helpTitle}>
                <HelperWidget />
            </Card>
            {can('station.view') && (
                <Card id="watch" title={tr.helperPc.watchTitle}>
                    <Watch opened={placement} onOpen={() => setPlacement((open) => !open)} />
                </Card>
            )}
            {placement && can('station.view') && <PlacementCard />}
            <Updates />
            {can('models.manage') && (
                <section class="card glass card-list" aria-label={tr.helperPc.modelsLabel}>
                    <div class="w-rows">
                        <button class="w-row w-row-btn" aria-expanded={models === 'recognition'} onClick={() => setModels(models === 'recognition' ? null : 'recognition')}>
                            <span class="flex-1 min-w-0 flex flex-col">
                                <span class="w-text">{tr.helperPc.hearingTitle}</span>
                                <span class="w-cap">{tr.helperPc.hearingAbout}</span>
                            </span>
                        </button>
                        <button class="w-row w-row-btn" aria-expanded={models === 'voice'} onClick={() => setModels(models === 'voice' ? null : 'voice')}>
                            <span class="flex-1 min-w-0 flex flex-col">
                                <span class="w-text">{tr.helperPc.voicesTitle}</span>
                                <span class="w-cap">{tr.helperPc.voicesAbout}</span>
                            </span>
                        </button>
                    </div>
                </section>
            )}
            {models && <ModelsScreen key={models} tab={models} onTab={() => undefined} bare />}
        </>
    );
}

/** Фрейм страницы компьютера в пульте колонки: без рамки, высотой с окно, прокрутка — внутри. */
export function OwnFrame({ page, title }: { page: 'helper' | 'workshop'; title: string }) {
    return <iframe class="own-frame" src={`${location.pathname}?own=${page}`} title={title} />;
}
