import { useEffect, useState } from 'preact/hooks';

export interface WallTime {
    hours: number;
    minutes: number;
}

/** Часы и минуты в часовом поясе колонки; пояс незнаком браузеру — его собственное время. */
export function timeIn(date: Date, timeZone?: string): WallTime {
    if (timeZone) {
        try {
            const parts = new Intl.DateTimeFormat('en-GB', {
                timeZone,
                hour: '2-digit',
                minute: '2-digit',
                hourCycle: 'h23',
            }).formatToParts(date);
            const hours = Number(parts.find((part) => part.type === 'hour')?.value);
            const minutes = Number(parts.find((part) => part.type === 'minute')?.value);
            if (Number.isFinite(hours) && Number.isFinite(minutes)) return { hours, minutes };
        } catch {
            // незнакомый пояс: время браузера
        }
    }
    return { hours: date.getHours(), minutes: date.getMinutes() };
}

/** Время для часов на корпусе: перерисовка ровно на смене минуты, а не опросом. */
export function useWallTime(timeZone?: string): WallTime {
    const [now, setNow] = useState(() => new Date());
    useEffect(() => {
        let timer = 0;
        const schedule = () => {
            timer = window.setTimeout(
                () => {
                    setNow(new Date());
                    schedule();
                },
                60_000 - (Date.now() % 60_000) + 50,
            );
        };
        schedule();
        return () => window.clearTimeout(timer);
    }, []);
    return timeIn(now, timeZone);
}
