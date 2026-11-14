// Плитка «Подключено…» на столе (docs/plans/shared-folders.md, решение владельца 3): флешку воткнули — «Посмотреть,
// что на ней?»; папку бросили на окно DuoWave — «Подключить?». Плитка есть всегда, даже когда колонка спросила голосом:
// в тихом режиме колонка промолчит, а плитка останется. Флешку открывает любой из семьи; папку с диска — администратор.

import { useEffect, useState } from 'preact/hooks';

import { FolderOffer, api, can, problemOf } from '../api';
import { tr } from '../i18n';
import { foldersChanged, useFoldersVersion } from '../folders/state';
import { IconFolder } from '../icons';
import { usePanel } from '../shell/context';
import { Action } from '../ui/cards';
import './folders.css';

/** Что предлагают подключить сейчас — и что из этого можно этому входу: флешку любому из семьи, папку администратору. */
export function useFolderOffers(): FolderOffer[] {
    const version = useFoldersVersion();
    const [offers, setOffers] = useState<FolderOffer[]>([]);
    useEffect(() => {
        if (!can('station.view')) return;
        let alive = true;
        api.folders()
            .then((view) => alive && setOffers(view.offers))
            .catch(() => alive && setOffers([]));
        return () => {
            alive = false;
        };
    }, [version]);
    return offers.filter((offer) => offer.kind === 'drive' || can('plugins.manage'));
}

export function FolderOffers({ offers }: { offers: FolderOffer[] }) {
    const panel = usePanel();
    const [busy, setBusy] = useState<string | null>(null);
    const [error, setError] = useState<string | null>(null);
    const shown = offers.filter((offer) => offer.kind === 'drive' || can('plugins.manage'));
    if (shown.length === 0) return null;
    const answer = async (offer: FolderOffer, look: boolean): Promise<void> => {
        setBusy(offer.id);
        setError(null);
        try {
            if (look) {
                const rule = await api.folderLook(offer.id);
                panel.go('folders', rule.address);
            } else {
                await api.folderSkip(offer.id);
            }
            foldersChanged();
        } catch (e) {
            setError(problemOf(e));
        } finally {
            setBusy(null);
        }
    };
    return (
        <>
            {shown.map((offer) => (
                <section key={offer.id} class="card glass folder-offer" aria-label={offer.name}>
                    <span class="folders-icon" aria-hidden="true">
                        <IconFolder size={22} />
                    </span>
                    <span class="folder-offer-text flex flex-col">
                        <b>{offer.kind === 'drive' ? tr.folders.offerDrive(offer.name.toLowerCase().startsWith('флешка') ? offer.name.replace(/^Флешка/, 'флешка') : tr.folders.offerDriveNamed(offer.name)) : tr.folders.offerFolder(offer.name)}</b>
                        <span class="w-cap">
                            {offer.kind === 'drive'
                                ? tr.folders.offerDriveText
                                : tr.folders.offerFolderText}
                        </span>
                        {error && busy === null && (
                            <span class="t-caption c-dot-offline" role="alert">
                                {error}
                            </span>
                        )}
                    </span>
                    <span class="flex gap-8">
                        <Action tone="on" label={offer.kind === 'drive' ? tr.folders.offerLook : tr.folders.offerAttach} busy={busy === offer.id} onClick={() => void answer(offer, true)} />
                        <Action label={tr.folders.offerSkip} disabled={busy === offer.id} onClick={() => void answer(offer, false)} />
                    </span>
                </section>
            ))}
        </>
    );
}
