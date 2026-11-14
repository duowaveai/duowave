// Texts: screens/station-widgets.tsx (the speaker's timers and music on the desk).
import type { Translation } from '../../types';
import type ru from '../ru/stationWidgets';

const en: Translation<typeof ru> = {
    reading: 'Reading…',
    copyFrom: (when: string) => `copy from ${when}`,
    noTimersCopy: (copy: string) => `There were no timers (${copy}).`,
    noTimers: 'No timers. Tell the speaker: “set a tea timer for five minutes”.',
    copyOffline: (copy: string) => `${copy} — the speaker is offline`,
    timer: 'Timer',
    paused: ' · paused',
    elapsed: (percent: number) => `${percent} percent elapsed`,
    justNow: 'just now',
    minutes: (n: number) => `${n} min`,
    hoursMinutes: (hours: number, minutes: number) => `${hours} h ${minutes} min`,
    music: 'Music',
    nothingPlayedCopy: (copy: string) => `Nothing was playing (${copy}).`,
    nothingPlaying: 'Nothing is playing right now. Say: “play the radio” or “play some music”.',
    musicLabel: (playing: boolean, title: string) => `Music: “${title}” ${playing ? 'is playing' : 'is stopped'}`,
    playingOnSpeaker: 'playing on the speaker',
    onPause: 'paused',
    stopped: 'stopped',
    like: 'Like',
    onAir: (time: string) => `on air for ${time}`,
    yandexPlays: 'Yandex is playing',
    onAirNow: 'on air',
    listened: (done: string, total: string) => `Played ${done} of ${total}`,
    previous: 'Previous',
    next: 'Next',
    stop: 'Stop',
    pause: 'Pause',
    play: 'Play',
};

export default en;
