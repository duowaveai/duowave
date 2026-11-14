// Texts: screens/news.tsx.
import type { Translation } from '../../types';
import type ru from '../ru/news';

const en: Translation<typeof ru> = {
    opmlTitleSpeaker: 'Speaker news',
    opmlTitleOwn: 'DuoWave news',
    noNewFeeds: 'There are no new feeds in the file.',
    title: 'News',
    reading: 'Reading settings…',
    checkBusy: 'Opening feeds…',
    check: 'Check feeds',
    done: 'Done',
    aboutSpeaker:
        '“Alice, what’s new?” — the speaker reads the headlines of these feeds and the model retells them. “Sports news” and other catalog topics use the feeds of that topic. Without feeds, news stays with Yandex.',
    aboutOwn:
        '“What’s new?” — DuoWave reads the headlines of these feeds and the model retells them. “Sports news” and other catalog topics use the feeds of that topic. Without feeds, there is no news.',
    yourFeeds: 'Your feeds',
    yourFeedsCount: (on: number, all: number) => ` · ${on} of ${all}`,
    noFeeds: 'None yet: add one from the catalog below, or your own by address.',
    feedLabel: (name: string) => `Feed ${name}`,
    remove: 'remove',
    catalog: 'Catalog',
    topics: 'Catalog topics',
    readingCatalog: 'Reading the catalog…',
    added: 'added',
    add: 'add',
    catalogNote: 'The catalog is built from open lists (awesome-rss-feeds, Wikidata) and the publications’ own feeds; each was checked from home.',
    ownFeed: 'Your own feed',
    addressLabel: 'RSS or Atom feed address',
    addressPlaceholder: 'https://… — RSS or Atom address',
    look: 'preview',
    headlines: (count: number, first: string) => `headlines now: ${count}. First ones: ${first}`,
    topicLabel: 'Feed topic',
    noTopic: 'no topic',
    alreadyHave: 'already added',
    opmlNote: 'Feeds from another reader — as an OPML file, and back.',
    opmlReading: 'Reading…',
    opmlImport: 'import OPML',
    opmlExport: 'export OPML',
};

export default en;
