// Права приложения словами — в каталоге до установки и в листе «Разрешить всё» окна приложения (этап 9 плана «Ленты»).
// Слова — из словаря (`tr.share.right*`), читаются при обращении: язык меняется без перезагрузки.

import { tr } from '../i18n';

function words(right: string): string | undefined {
    switch (right) {
        case 'call:system/brain':
            return tr.share.rightBrain;
        case 'call:system/voice':
            return tr.share.rightVoice;
        case 'call:system/voice/cloud':
            return tr.share.rightVoiceCloud;
        case 'call:system/notify':
            return tr.share.rightNotify;
        case 'call:system/schedule':
            return tr.share.rightSchedule;
        case 'call:system/memory':
            return tr.share.rightMemory;
        case 'call:system/publish':
            return tr.share.rightPublish;
        case 'call:system/hearing':
            return tr.share.rightHearing;
        case 'call:system/smart-home':
            return tr.share.rightSmartHome;
        case 'call:system/content':
            return tr.share.rightContent;
        default:
            return undefined;
    }
}

/** Одно право словами: «модель «Мозга»: пишет тексты», «сеть: api.example.com». */
export function rightWords(right: string): string {
    return right.startsWith('net:') ? tr.share.rightNet(right.slice(4)) : words(right) ?? right;
}
