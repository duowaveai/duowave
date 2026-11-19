// Места виджетов на свободном столе (баг-лист владельца 19.11, группа Б). Было: виджеты, которых не двигали руками,
// стол на каждом шаге раскладывал по столбцам заново — по высотам. Поставили или убрали виджет, у соседа пришли данные,
// заиграла музыка — соседи переезжали в другой столбец («прыгают влево-вправо»). А столбцы не видели виджетов со своим
// местом: «Fresh Hip-Hop & Rap» сел на место «Новый виджет», пока музыка молчала, заиграла — музыка встала в столбец
// туда же, один под другим.
//
// Теперь у каждого виджета своё место, как только он появился: первое свободное — в том же порядке, что раньше
// (столбец слева от колонки, справа, дальние), — и оно хранится в раскладке. Новый не сдвигает соседей, убранный
// оставляет пусто. Вырос (пришли данные) и лёг на соседа — сосед отходит вниз, ровно насколько нужно. Здесь — только
// счёт, без страницы: его проверяет scripts/panel-check.ts.

export interface Rect {
    x: number;
    y: number;
    w: number;
    h: number;
}

/** Пересекаются ли (с зазором `gap` вокруг `b`). Касание краями — не пересечение. */
export function overlaps(a: Rect, b: Rect, gap = 0): boolean {
    return a.x < b.x + b.w + gap && b.x < a.x + a.w + gap && a.y < b.y + b.h + gap && b.y < a.y + a.h + gap;
}

export interface FreeInput {
    /** Левые края столбцов по порядку: ближний слева, ближний справа, дальний слева, дальний справа. */
    columns: number[];
    width: number;
    /** Выше этого столбцы не начинаются (верх сцены). */
    top: number;
    /** Что уже занято: сцена, витрина под ней, виджеты. */
    taken: Rect[];
    /** Зазор между виджетами. */
    gap: number;
    /** Что видно в начале стола: левый и правый край, низ (над доком), точки стола. */
    sight: { left: number; right: number; bottom: number };
}

/** Самое верхнее свободное место высотой `h` в столбце `x`. */
function freeIn(x: number, h: number, input: FreeInput): number {
    const { width, top, taken, gap } = input;
    const near = taken.filter((rect) => rect.x < x + width + gap && x < rect.x + rect.w + gap);
    const tries = [top, ...near.map((rect) => rect.y + rect.h + gap)].filter((y) => y >= top).sort((a, b) => a - b);
    for (const y of tries) if (!near.some((rect) => overlaps({ x, y, w: width, h }, rect, gap))) return y;
    // Ниже всего занятого в этом столбце место есть всегда; сюда не дойдёт.
    return Math.max(top, ...near.map((rect) => rect.y + rect.h + gap));
}

/**
 * Свободное место для виджета высотой `h`: сначала там, где его видно в начале стола целиком (не за краем окна и не
 * ниже дока), из таких — самое верхнее; не видно нигде — самое верхнее в любом столбце (стол растёт вширь, в дальние
 * столбцы сверху, а не вниз под витрину и док), при равных — ближний.
 */
export function freeSpot(h: number, input: FreeInput): { x: number; y: number } {
    const { columns, width, sight } = input;
    const spots = columns.map((x, index) => ({ x, y: freeIn(x, h, input), index }));
    const highest = (list: typeof spots) => [...list].sort((a, b) => a.y - b.y || a.index - b.index)[0];
    const seen = spots.filter((spot) => spot.x >= sight.left && spot.x + width <= sight.right && spot.y + h <= sight.bottom);
    const pick = highest(seen) ?? highest(spots);
    return { x: pick.x, y: pick.y };
}

/** Виджет на столе для расстановки: ключ, место (если есть) и высота сейчас; 0 — не виден (пустой прячется). */
export interface Laid {
    key: string;
    place?: { x: number; y: number; w?: number; h?: number };
    /** Высота сейчас; нет — ещё не измерен. */
    height?: number;
}

/** Сколько виджет занимает: видимый — свою высоту; спрятанный, пока пуст, — высоту, с которой место заняли. */
export function heldHeight(widget: Laid, guess: number): number {
    if (widget.height === undefined) return widget.place?.h ?? guess;
    return widget.height > 0 ? widget.height : (widget.place?.h ?? 0);
}

export interface Arranged {
    /** Где стоит каждый (у ещё не измеренных — где встанет). */
    spots: Record<string, { x: number; y: number; w: number }>;
    /** Новые места: измеренным без места — первое свободное; на них стол записывает их в раскладку. */
    fresh: Record<string, { x: number; y: number; w: number; h: number }>;
    /** Место «Новый виджет»: где встанет следующий. */
    add: { x: number; y: number };
}

/**
 * Расставить: у кого место есть — там; новые (без места) — по порядку на первое свободное. Спрятанные, пока пусты, и
 * не стоявшие ни разу места не берут; стоявшие — держат своё. «Новый виджет» — на первое свободное после всех.
 */
export function arrange(
    widgets: Laid[],
    fixed: Rect[],
    input: Omit<FreeInput, 'taken'> & { guess: number; addHeight: number; keep?: { x: number; y: number } | null; cover?: Rect | null },
): Arranged {
    // `guess` — и высота ещё не измеренного, и наименьшее место, что ищется новому. `keep` — где «Новый виджет» стоял
    // до сих пор: пока там свободно и видно, он стоит там же (не прыгает, когда освободилось место выше или закрыли
    // карточку Турбо), и новый виджет встаёт туда — где была плитка. У стола, где мест ещё нет, — не в счёт.
    // `cover` — что висит поверх стола (карточка Турбо), в точках стола: под ним новые не встают — их там не видно.
    const { width, guess, addHeight, sight, gap, cover } = input;
    const taken: Rect[] = [...fixed, ...(cover ? [cover] : [])];
    let keep = widgets.some((widget) => widget.place) ? (input.keep ?? null) : null;
    const free = (spot: { x: number; y: number }, h: number) => !taken.some((rect) => overlaps({ ...spot, w: width, h }, rect, gap));
    const seen = (spot: { x: number; y: number }, h: number) => spot.x >= sight.left && spot.x + width <= sight.right && spot.y + h <= sight.bottom;
    const spots: Arranged['spots'] = {};
    const fresh: Arranged['fresh'] = {};
    for (const widget of widgets) {
        if (!widget.place) continue;
        const w = widget.place.w ?? width;
        spots[widget.key] = { x: widget.place.x, y: widget.place.y, w };
        const h = heldHeight(widget, guess);
        if (h > 0) taken.push({ x: widget.place.x, y: widget.place.y, w, h });
    }
    for (const widget of widgets) {
        if (widget.place) continue;
        // Пустой и прячется — места не берёт, пока не появится.
        if (widget.height === 0) continue;
        const h = widget.height ?? guess;
        // Место — не меньше `guess`: только что открытый виджет бывает ниже, чем станет, и в узкую щель не встаёт.
        const found = keep && free(keep, Math.max(h, guess)) ? keep : freeSpot(Math.max(h, guess), { ...input, taken });
        const spot = { x: Math.round(found.x), y: Math.round(found.y) };
        keep = null;
        spots[widget.key] = { ...spot, w: width };
        taken.push({ ...spot, w: width, h });
        if (widget.height !== undefined) fresh[widget.key] = { ...spot, w: width, h: Math.round(h) };
    }
    const add = keep && free(keep, addHeight) && seen(keep, addHeight) ? keep : freeSpot(addHeight, { ...input, taken });
    return { spots, fresh, add };
}

/**
 * Виджет вырос и лёг на соседа: кто ниже — отходит вниз, под того, на кого лёг, с зазором. Неподвижное (сцена, витрина)
 * не двигается — на него лёгший отходит под него. Возвращает новые места тех, кто отошёл.
 */
export function pushApart(rects: Array<Rect & { key: string }>, fixed: Rect[], gap: number): Record<string, { x: number; y: number }> {
    const order = [...rects].sort((a, b) => a.y - b.y || a.x - b.x);
    const done: Rect[] = [];
    const moved: Record<string, { x: number; y: number }> = {};
    for (const rect of order) {
        let y = rect.y;
        for (let guard = 0; guard < 100; guard++) {
            const here = { ...rect, y };
            const under = [...done, ...fixed].filter((other) => overlaps(here, other));
            if (under.length === 0) break;
            y = Math.max(...under.map((other) => other.y + other.h)) + gap;
        }
        if (y !== rect.y) moved[rect.key] = { x: rect.x, y: Math.round(y) };
        done.push({ ...rect, y });
    }
    return moved;
}

/**
 * Где встаёт витрина под сценой (широкий виджет `under`): на своём месте под сценой, но не на виджеты, что начинаются
 * выше этого места и заходят на него (стояли в ближнем столбце, пока строки под колонкой были длиннее), — под ними.
 * Кто начинается ниже и лёг на неё — отходит сам (pushApart).
 */
export function underTop(base: number, half: number, rects: Rect[], gap: number): number {
    let top = base;
    for (const rect of rects) if (rect.x < half && rect.x + rect.w > -half && rect.y < base && rect.y + rect.h + gap > top) top = rect.y + rect.h + gap;
    return top;
}
