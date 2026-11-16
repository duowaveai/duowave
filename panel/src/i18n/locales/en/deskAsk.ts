// Texts of the “Put it on the desk?” line under the scene (screens/desk-ask.tsx).
import type { Translation } from '../../types';
import type ru from '../ru/deskAsk';

const en: Translation<typeof ru> = {
    asked: (title: string, times: number) => `You asked about “${title}” ${times === 2 ? 'twice' : 'a few times'}. Put it on the desk?`,
    put: 'Put it there',
    no: 'No thanks',
};

export default en;
