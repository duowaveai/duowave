// Texts: screens/draft-tile.tsx, draft-preview.tsx, widget-agent.tsx, add-widget.tsx.
import type { Translation } from '../../types';
import type ru from '../ru/drafts';

const en: Translation<typeof ru> = {
    // draft-tile
    faceGame: 'game',
    faceTheme: 'theme',
    faceSkill: 'skill',
    faceWidget: 'widget',
    askedSpeakerLoading: 'Asked via the speaker. Loading the example…',
    askedVoiceLoading: 'Asked by voice. Loading the example…',
    loadingExample: 'Loading the example…',
    reading: 'Reading…',
    themeExample: 'A panel theme — you try it on.',
    noPhrases: 'No phrases yet.',
    newThing: 'new thing',
    slotLabel: (title: string) => `Place for “${title}”`,
    slotCap: 'its place',
    askedSpeaker: 'asked via the speaker',
    askedVoice: 'asked by voice',
    faceLabel: (word: string, title: string) => `${word} “${title}”`,
    onFitting: 'in try-on',
    failedWithNote: (note: string) => `It didn’t work. ${note}`,
    failedNoteDefault: 'What happened is in the workshop.',
    stoppedNote: 'Stopped. What the agent finished is kept in the draft.',
    thinking: 'thinking',
    sampleData: 'sample data',
    failedShort: 'failed',
    stoppedShort: 'stopped',
    andMore: (n: number) => `and ${n} more`,
    tryOn: 'Try on',
    toDesk: 'To the desk',
    enable: 'Turn on',
    orSayIt: 'or say it',
    redoEllipsis: 'Redo…',
    toDrafts: 'To drafts',
    redoDefault: 'Redo it',
    redoPlaceholder: 'What to change: bigger, another source…',
    redoLabel: 'What to change',
    handing: 'Sending…',
    redo: 'Redo',
    newWidget: 'New widget',
    newWidgetCap: 'say what to show, and I’ll draw it',

    // draft-preview
    callFailed: (fn: string) => `${fn} failed`,
    codeChanged: 'panel.js changed while the gateway was answering — press “Show again”.',
    noDeskWidget: 'The draft has no desk widget (the desk.widgets slot).',
    loadingWidget: 'Loading the widget…',

    // widget-agent
    askedSpeakerHint: 'or “Say” in the dock, or to the speaker: “Alice, make a widget…”',
    askedVoiceHint: 'or “Say” in the dock: “Make a widget…”',
    adminOnly: 'A new widget is drawn by the workshop agent — an administrator can ask for it.',
    askTitle: 'Say or write what to show',
    askPlaceholder: 'My tasks for today from Todoist — with the option to tick them off',
    askLabel: 'What to show on the widget',
    handingToAgent: 'Handing it to the agent…',
    draw: 'Draw',
    appearsAsTile: 'It will appear on the desk as a tile — you can watch the agent make it, with sample data.',

    // add-widget
    close: 'Close',
    addTitle: 'New widget',
    addCap: 'by voice or text — or a ready one',
    readyHeading: 'Ready',
    readyAllOnDesk: 'Ready — all are already on the desk',
    builtIn: 'built-in',
    app: 'app',
    sizeLarge: 'large',
    sizeSmall: 'small',
    put: 'Add',
    catalog: 'Catalog',
    catalogCap: 'apps with widgets: weather, radio, exchange rates, a shopping list…',
    dragHint: 'Move a widget on the desk by its header and resize it by the corner.',
};

export default en;
