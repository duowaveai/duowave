// Texts: common words for every screen.

import { plural } from '../../format';
import type { Translation } from '../../types';
import type ru from '../ru/common';

const en: Translation<typeof ru> = {
    save: 'Save',
    cancel: 'Cancel',
    close: 'Close',
    delete: 'Delete',
    remove: 'Remove',
    add: 'Add',
    edit: 'Edit',
    retry: 'Try again',
    back: 'Back',
    next: 'Next',
    done: 'Done',
    yes: 'Yes',
    no: 'No',
    ok: 'OK',
    on: 'On',
    off: 'Off',
    enable: 'Turn on',
    disable: 'Turn off',
    loading: 'Loading…',
    wait: 'One moment…',
    saved: 'Saved',
    error: 'Error',
    errorCode: (status: number) => `Error ${status}`,
    gatewayUnavailable: 'The gateway is unreachable — please try again.',
    notFromGateway: 'The file is not from the gateway',
    stationAnswersAgain: 'The speaker is answering again — please try again.',
    more: 'More',
    show: 'Show',
    hide: 'Hide',
    open: 'Open',
    copy: 'Copy',
    copied: 'Copied',
    search: 'Search',
    nothingFound: 'Nothing found',
    language: 'Language',
    minutes: (n: number) => `${n} ${plural(n, { one: 'minute', other: 'minutes' })}`,
    seconds: (n: number) => `${n} ${plural(n, { one: 'second', other: 'seconds' })}`,
    hours: (n: number) => `${n} ${plural(n, { one: 'hour', other: 'hours' })}`,
    days: (n: number) => `${n} ${plural(n, { one: 'day', other: 'days' })}`,
};

export default en;
