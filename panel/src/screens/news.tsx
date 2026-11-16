import { useEffect, useState } from 'preact/hooks';

import { ConnectorSettings, FeedPreview, NewsCatalog, NewsFeedEntry, api, problemOf } from '../api';
import { tr } from '../i18n';
import { BrandIconCircle } from '../ui/brand';
import { CheckLine, EnvNote } from '../ui/fields';
import { Sheet, SkeletonRows, WinRow, WinSection } from '../ui/sheet';
import { Switch } from '../ui/switch';
import { byMode } from '../words';

/** Слово с большой буквы — для кнопок окна (строки словаря строчные). */
const cap = (text: string): string => text.charAt(0).toUpperCase() + text.slice(1);

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
    const spin = (on: boolean) => on && <span class="spinner spinner-sm" aria-hidden="true" />;
    return (
        <Sheet
            title={tr.news.title}
            about={view?.status.detail || undefined}
            icon={<BrandIconCircle id="rss" />}
            onClose={onClose}
            footer={
                <button
                    class="btn btn-plain"
                    disabled={busy !== null || feeds.length === 0}
                    aria-busy={busy === 'check' || undefined}
                    onClick={() => void act('check', () => api.checkConnector('news'))}
                >
                    {busy === 'check' ? tr.news.checkBusy : tr.news.check}
                </button>
            }
        >
            <p class="win-lead">{byMode(tr.news.aboutSpeaker, tr.news.aboutOwn)}</p>
            {view && <EnvNote fromEnv={view.fromEnv} envLeft={view.envLeft} />}
            <CheckLine check={view?.check} />

            <WinSection title={tr.news.yourFeeds} aside={feeds.length ? tr.news.yourFeedsAside(feeds.filter((feed) => feed.enabled).length, feeds.length) : undefined}>
                {!view ? (
                    <SkeletonRows rows={2} />
                ) : (
                    <div class="win-group">
                        {!feeds.length && (
                            <div class="win-row">
                                <span class="win-row-s">{tr.news.noFeeds}</span>
                            </div>
                        )}
                        {feeds.map((feed) => {
                            const item = itemOf(feed.url);
                            return (
                                <WinRow
                                    key={feed.url}
                                    title={feed.title ?? hostOf(feed.url)}
                                    sub={
                                        <>
                                            {[topicTitle(feed.topic), hostOf(feed.url)].filter(Boolean).join(' · ')}
                                            {item && <span class={`ml-6 ${item.ok ? 'c-accent-text' : 'c-dot-offline'}`}>· {item.text}</span>}
                                        </>
                                    }
                                >
                                    <button
                                        class="btn btn-quiet btn-sm"
                                        disabled={busy !== null}
                                        onClick={() => save(`off:${feed.url}`, feeds.filter((other) => other.url !== feed.url))}
                                    >
                                        {cap(tr.news.remove)}
                                    </button>
                                    <Switch
                                        on={feed.enabled}
                                        label={tr.news.feedLabel(feed.title ?? hostOf(feed.url))}
                                        disabled={busy !== null}
                                        onChange={(on) => save(`on:${feed.url}`, feeds.map((other) => (other.url === feed.url ? { ...other, enabled: on } : other)))}
                                    />
                                </WinRow>
                            );
                        })}
                    </div>
                )}
            </WinSection>

            <WinSection title={tr.news.catalog}>
                {!catalog ? (
                    <SkeletonRows rows={3} label={tr.news.readingCatalog} />
                ) : (
                    <>
                        <div role="group" aria-label={tr.news.topics} class="win-tags">
                            {catalog.topics.map((entry) => (
                                <button
                                    key={entry.id}
                                    class={`pill ${entry.id === topic ? 'pill-pick' : 'pill-off'} pill-btn-sm`}
                                    aria-pressed={entry.id === topic}
                                    onClick={() => setTopic(entry.id)}
                                >
                                    {entry.title}
                                </button>
                            ))}
                        </div>
                        {shown.length > 0 && (
                            <div class="win-group">
                                {shown.map((feed) => {
                                    const added = has(feed.url);
                                    return (
                                        <WinRow key={feed.id} title={feed.title} sub={feed.site}>
                                            {added ? (
                                                <span class="pill pill-own">{tr.news.added}</span>
                                            ) : (
                                                <button
                                                    class="btn btn-plain btn-sm"
                                                    disabled={busy !== null}
                                                    aria-busy={busy === `add:${feed.id}` || undefined}
                                                    onClick={() =>
                                                        save(`add:${feed.id}`, [...feeds, { url: feed.url, title: feed.title, topic: feed.topic, catalog: feed.id, enabled: true }])
                                                    }
                                                >
                                                    {spin(busy === `add:${feed.id}`)}
                                                    {cap(tr.news.add)}
                                                </button>
                                            )}
                                        </WinRow>
                                    );
                                })}
                            </div>
                        )}
                    </>
                )}
                <p class="win-note">{tr.news.catalogNote}</p>
            </WinSection>

            <WinSection title={tr.news.ownFeed}>
                <form
                    class="flex gap-8 items-center"
                    onSubmit={(e) => {
                        e.preventDefault();
                        if (address.trim()) look();
                    }}
                >
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
                    <button type="submit" class="btn btn-plain" disabled={!address.trim() || busy !== null} aria-busy={busy === 'look' || undefined}>
                        {spin(busy === 'look')}
                        {cap(tr.news.look)}
                    </button>
                </form>
                {preview && (
                    <div class="win-group">
                        <div class="win-row is-col">
                            <div class="win-row-main">
                                <span class="win-row-t">{preview.title ?? hostOf(preview.url)}</span>
                                <span class="win-row-s">{tr.news.headlines(preview.count, preview.headlines.slice(0, 3).join(' · '))}</span>
                            </div>
                            <div class="flex gap-8 items-center flex-wrap">
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
                                <button
                                    class="btn btn-plain btn-sm"
                                    disabled={has(preview.url) || busy !== null}
                                    onClick={() => {
                                        save('own', [...feeds, { url: preview.url, title: preview.title, topic: previewTopic || undefined, enabled: true }]);
                                        setPreview(null);
                                        setAddress('');
                                    }}
                                >
                                    {has(preview.url) ? cap(tr.news.alreadyHave) : cap(tr.news.add)}
                                </button>
                            </div>
                        </div>
                    </div>
                )}
            </WinSection>

            <WinSection group>
                <div class="win-row is-col">
                    <span class="win-row-s">{tr.news.opmlNote}</span>
                    <div class="flex gap-8 flex-wrap">
                        <label class="btn btn-plain btn-sm" aria-busy={busy === 'opml' || undefined}>
                            {spin(busy === 'opml')}
                            {busy === 'opml' ? tr.news.opmlReading : cap(tr.news.opmlImport)}
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
                        <button class="btn btn-plain btn-sm" disabled={!feeds.length} onClick={exportOpml}>
                            {cap(tr.news.opmlExport)}
                        </button>
                    </div>
                </div>
            </WinSection>

            {error && (
                <p role="alert" class="win-callout m-0">
                    {error}
                </p>
            )}
        </Sheet>
    );
}
