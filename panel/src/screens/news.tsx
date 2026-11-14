import { useEffect, useState } from 'preact/hooks';

import { ConnectorSettings, FeedPreview, NewsCatalog, NewsFeedEntry, api, problemOf } from '../api';
import { tr } from '../i18n';
import { BrandIconCircle } from '../ui/brand';
import { Action } from '../ui/cards';
import { CheckLine, EnvNote } from '../ui/fields';
import { Sheet } from '../ui/sheet';
import { Switch } from '../ui/switch';
import { byMode } from '../words';

/** Ленты в том виде, что ждёт шлюз: адрес, имя, тема и включена ли. */
const asInput = (feeds: NewsFeedEntry[]) => feeds.map(({ url, title, topic, enabled }) => ({ url, title, topic, enabled }));

const hostOf = (url: string): string => {
    try {
        return new URL(url).host;
    } catch {
        return url;
    }
};

const sameFeed = (a: string, b: string): boolean => {
    const plain = (url: string) => url.trim().replace(/^[a-z]+:\/\//i, '').replace(/\/+$/, '').toLowerCase();
    return plain(a) === plain(b);
};

/** Ленты из OPML любой читалки: адрес и имя каждой. */
function opmlFeeds(text: string): Array<{ url: string; title?: string }> {
    const doc = new DOMParser().parseFromString(text, 'text/xml');
    return Array.from(doc.querySelectorAll('outline[xmlUrl]')).map((outline) => ({
        url: outline.getAttribute('xmlUrl') ?? '',
        title: outline.getAttribute('title') ?? outline.getAttribute('text') ?? undefined,
    }));
}

function opmlOf(feeds: NewsFeedEntry[]): string {
    const escape = (text: string) => text.replace(/&/g, '&amp;').replace(/"/g, '&quot;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
    const outlines = feeds
        .map((feed) => {
            const title = escape(feed.title ?? hostOf(feed.url));
            return `    <outline type="rss" text="${title}" title="${title}" xmlUrl="${escape(feed.url)}"/>`;
        })
        .join('\n');
    return `<?xml version="1.0" encoding="UTF-8"?>\n<opml version="2.0">\n  <head><title>${byMode(tr.news.opmlTitleSpeaker, tr.news.opmlTitleOwn)}</title></head>\n  <body>\n${outlines}\n  </body>\n</opml>\n`;
}

/**
 * «Новости»: ленты, из которых колонка читает заголовки. Каталог по темам (проверенные ленты), своя лента по адресу
 * (сначала видно, что в ней), OPML туда и обратно. Каждое изменение применяется сразу; «Проверить» открывает все.
 */
export function NewsSheet({ onClose, onChanged }: { onClose: () => void; onChanged: () => void }) {
    const [view, setView] = useState<ConnectorSettings | null>(null);
    const [catalog, setCatalog] = useState<NewsCatalog | null>(null);
    const [topic, setTopic] = useState('main');
    const [address, setAddress] = useState('');
    const [preview, setPreview] = useState<FeedPreview | null>(null);
    const [previewTopic, setPreviewTopic] = useState('');
    const [busy, setBusy] = useState<string | null>(null);
    const [error, setError] = useState<string | null>(null);

    useEffect(() => {
        api.connectorSettings('news')
            .then(setView)
            .catch((e) => setError(problemOf(e)));
        api.newsCatalog()
            .then(setCatalog)
            .catch(() => setCatalog({ topics: [], feeds: [] }));
    }, []);

    const feeds: NewsFeedEntry[] = Array.isArray(view?.values.feeds) ? (view!.values.feeds as NewsFeedEntry[]) : [];
    const topicTitle = (id?: string) => catalog?.topics.find((entry) => entry.id === id)?.title;
    const itemOf = (url: string) => view?.check?.items?.find((item) => item.key === url);

    const act = async (key: string, work: () => Promise<ConnectorSettings | void>) => {
        setBusy(key);
        setError(null);
        try {
            const fresh = await work();
            if (fresh) {
                setView(fresh);
                onChanged();
            }
        } catch (e) {
            setError(problemOf(e));
        } finally {
            setBusy(null);
        }
    };
    const save = (key: string, next: NewsFeedEntry[]) => void act(key, () => api.updateConnector('news', { feeds: asInput(next) }));
    const has = (url: string) => feeds.some((feed) => sameFeed(feed.url, url));

    const look = () =>
        void act('look', async () => {
            setPreview(null);
            const found = await api.previewFeed(address.trim());
            setPreview(found);
            const known = catalog?.feeds.find((feed) => sameFeed(feed.url, found.url));
            setPreviewTopic(known?.topic ?? '');
        });

    const importOpml = async (file: File) => {
        const found = opmlFeeds(await file.text()).filter((feed) => feed.url && !has(feed.url));
        if (!found.length) {
            setError(tr.news.noNewFeeds);
            return;
        }
        await act('opml', () =>
            api.updateConnector('news', {
                feeds: [...asInput(feeds), ...found.map((feed) => ({ url: feed.url, title: feed.title, enabled: true }))],
            }),
        );
    };

    const exportOpml = () => {
        const blob = new Blob([opmlOf(feeds)], { type: 'text/x-opml' });
        const link = document.createElement('a');
        link.href = URL.createObjectURL(blob);
        link.download = 'news.opml';
        link.click();
        setTimeout(() => URL.revokeObjectURL(link.href), 1000);
    };

    const shown = catalog?.feeds.filter((feed) => feed.topic === topic) ?? [];
    return (
        <Sheet
            title={tr.news.title}
            about={view ? (view.status.detail ?? '') : tr.news.reading}
            icon={<BrandIconCircle id="rss" />}
            onClose={onClose}
            footer={
                <div class="flex gap-8 flex-wrap">
                    <button
                        class="pill pill-own dash-confirm-btn"
                        disabled={busy !== null || feeds.length === 0}
                        aria-busy={busy === 'check'}
                        onClick={() => void act('check', () => api.checkConnector('news'))}
                    >
                        {busy === 'check' ? tr.news.checkBusy : tr.news.check}
                    </button>
                    <button class="pill pill-off dash-confirm-btn" onClick={onClose}>
                        {tr.news.done}
                    </button>
                </div>
            }
        >
            <p class="t-meta m-0">
                {byMode(tr.news.aboutSpeaker, tr.news.aboutOwn)}
            </p>
            {view && <EnvNote fromEnv={view.fromEnv} envLeft={view.envLeft} />}
            <CheckLine check={view?.check} />

            <div class="flex flex-col">
                <b class="my-4 mx-0">{tr.news.yourFeeds}{feeds.length ? tr.news.yourFeedsCount(feeds.filter((feed) => feed.enabled).length, feeds.length) : ''}</b>
                {view && !feeds.length && (
                    <span class="t-meta">{tr.news.noFeeds}</span>
                )}
                {feeds.map((feed) => {
                    const item = itemOf(feed.url);
                    return (
                        <div
                            key={feed.url}
                            class="flex items-center gap-10 py-8 px-0 bt-divider"
                        >
                            <Switch
                                on={feed.enabled}
                                label={tr.news.feedLabel(feed.title ?? hostOf(feed.url))}
                                disabled={busy !== null}
                                onChange={(on) => save(`on:${feed.url}`, feeds.map((other) => (other.url === feed.url ? { ...other, enabled: on } : other)))}
                            />
                            <span class="flex-1 min-w-0 flex flex-col">
                                <b class="fs-14 overflow-hidden text-ellipsis nowrap">{feed.title ?? hostOf(feed.url)}</b>
                                <span class="t-caption wrap-anywhere">
                                    {[topicTitle(feed.topic), hostOf(feed.url)].filter(Boolean).join(' · ')}
                                    {item && (
                                        <span class={`ml-6 ${item.ok ? 'c-accent-text' : 'c-dot-offline'}`}>· {item.text}</span>
                                    )}
                                </span>
                            </span>
                            <Action
                                label={tr.news.remove}
                                disabled={busy !== null}
                                onClick={() => save(`off:${feed.url}`, feeds.filter((other) => other.url !== feed.url))}
                            />
                        </div>
                    );
                })}
            </div>

            <div class="flex flex-col gap-8">
                <b>{tr.news.catalog}</b>
                <div role="group" aria-label={tr.news.topics} class="flex gap-6 flex-wrap">
                    {catalog?.topics.map((entry) => (
                        <button
                            key={entry.id}
                            class={`pill ${entry.id === topic ? 'pill-own' : 'pill-off'} pill-btn-sm`}
                            aria-pressed={entry.id === topic}
                            onClick={() => setTopic(entry.id)}
                        >
                            {entry.title}
                        </button>
                    ))}
                </div>
                {!catalog && <span class="t-meta">{tr.news.readingCatalog}</span>}
                {shown.map((feed) => {
                    const added = has(feed.url);
                    return (
                        <div key={feed.id} class="flex items-center gap-10 min-h-44">
                            <span class="flex-1 min-w-0 flex flex-col">
                                <b class="fs-14">{feed.title}</b>
                                <span class="t-caption">{feed.site}</span>
                            </span>
                            <Action
                                label={added ? tr.news.added : tr.news.add}
                                tone={added ? 'off' : 'on'}
                                disabled={added || busy !== null}
                                busy={busy === `add:${feed.id}`}
                                onClick={() =>
                                    save(`add:${feed.id}`, [...feeds, { url: feed.url, title: feed.title, topic: feed.topic, catalog: feed.id, enabled: true }])
                                }
                            />
                        </div>
                    );
                })}
                <span class="t-meta">
                    {tr.news.catalogNote}
                </span>
            </div>

            <form
                class="flex flex-col gap-8"
                onSubmit={(e) => {
                    e.preventDefault();
                    if (address.trim()) look();
                }}
            >
                <b>{tr.news.ownFeed}</b>
                <span class="flex gap-8 items-center">
                    <input
                        class="field flex-1 min-w-0"
                        type="url"
                        inputMode="url"
                        autocomplete="off"
                        aria-label={tr.news.addressLabel}
                        placeholder={tr.news.addressPlaceholder}
                        value={address}
                        onInput={(e) => {
                            setAddress((e.target as HTMLInputElement).value);
                            setPreview(null);
                        }}
                    />
                    <Action label={tr.news.look} busy={busy === 'look'} disabled={!address.trim() || busy !== null} onClick={look} />
                </span>
                {preview && (
                    <div class="dash-confirm flex flex-col gap-6 bg-icon-bg c-text">
                        <b>{preview.title ?? hostOf(preview.url)}</b>
                        <span class="t-caption">
                            {tr.news.headlines(preview.count, preview.headlines.slice(0, 3).join(' · '))}
                        </span>
                        <span class="flex gap-8 items-center flex-wrap">
                            <select
                                class="field flex-1-1-160"
                                aria-label={tr.news.topicLabel}
                                value={previewTopic}
                                onChange={(e) => setPreviewTopic((e.target as HTMLSelectElement).value)}
                            >
                                <option value="">{tr.news.noTopic}</option>
                                {catalog?.topics.map((entry) => (
                                    <option key={entry.id} value={entry.id}>
                                        {entry.title}
                                    </option>
                                ))}
                            </select>
                            <Action
                                label={has(preview.url) ? tr.news.alreadyHave : tr.news.add}
                                tone="on"
                                disabled={has(preview.url) || busy !== null}
                                onClick={() => {
                                    save('own', [...feeds, { url: preview.url, title: preview.title, topic: previewTopic || undefined, enabled: true }]);
                                    setPreview(null);
                                    setAddress('');
                                }}
                            />
                        </span>
                    </div>
                )}
            </form>

            <div class="flex gap-8 items-center flex-wrap">
                <span class="t-caption flex-1-1-100p">
                    {tr.news.opmlNote}
                </span>
                <label class="pill pill-off cursor-pointer py-8 px-14 min-h-36 inline-flex items-center">
                    {busy === 'opml' ? tr.news.opmlReading : tr.news.opmlImport}
                    <input
                        type="file"
                        accept=".opml,.xml,text/xml,text/x-opml"
                        class="hidden"
                        onChange={(e) => {
                            const file = (e.target as HTMLInputElement).files?.[0];
                            if (file) void importOpml(file);
                            (e.target as HTMLInputElement).value = '';
                        }}
                    />
                </label>
                <Action label={tr.news.opmlExport} disabled={!feeds.length} onClick={exportOpml} />
            </div>

            {error && (
                <p role="alert" class="t-meta m-0 c-dot-offline">
                    {error}
                </p>
            )}
        </Sheet>
    );
}
