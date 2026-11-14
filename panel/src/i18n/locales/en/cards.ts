// Texts: cards/*.
import { plural } from '../../format';
import type { Translation } from '../../types';
import type ru from '../ru/cards';

const en: Translation<typeof ru> = {
    lineLabel: (from: string, to: string, min: string, max: string) => `Line: from ${from} to ${to}, lowest ${min}, highest ${max}`,
    barsLabel: (values: string) => `Bars: ${values}`,
    ready: 'done',
    pictureMissing: (name?: string) => `Picture not found${name ? `: ${name}` : ''}`,
    fieldLabel: 'Card field',
    send: 'Send',
    nothingYet: 'Nothing to show yet.',
    notACard: (fn: string) => `The ${fn} function didn’t return a card.`,
    loading: 'Loading…',
    fnFailed: (fn: string) => `${fn} didn’t finish`,
    loadingCard: 'Loading the card…',
};

export default en;
