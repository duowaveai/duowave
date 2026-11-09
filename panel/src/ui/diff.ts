// Что поменялось в тексте характера: по фразам, а не по строкам — промт обычно один абзац, и правка одного слова не
// должна перечёркивать его целиком. Наибольшая общая подпоследовательность фраз; промт до 4000 знаков — это десятки
// фраз, квадрат здесь не страшен.

export interface DiffPart {
    op: 'same' | 'del' | 'add';
    text: string;
}

/** Фразы текста: по концу предложения и по переносам строк. */
export function phrases(text: string): string[] {
    return text
        .split(/\n+/)
        .flatMap((line) => line.match(/[^.!?…]+[.!?…]*["»”)]*\s*|[.!?…]+\s*/g) ?? [])
        .map((part) => part.trim())
        .filter(Boolean);
}

/** Разница двух текстов по фразам: прежние, убранные и новые — в порядке нового текста. */
export function diffText(before: string, after: string): DiffPart[] {
    const a = phrases(before);
    const b = phrases(after);
    const n = a.length;
    const m = b.length;
    // lcs[i][j] — длина общей части a[i…] и b[j…].
    const lcs: number[][] = Array.from({ length: n + 1 }, () => new Array<number>(m + 1).fill(0));
    for (let i = n - 1; i >= 0; i--)
        for (let j = m - 1; j >= 0; j--) lcs[i][j] = a[i] === b[j] ? lcs[i + 1][j + 1] + 1 : Math.max(lcs[i + 1][j], lcs[i][j + 1]);
    const parts: DiffPart[] = [];
    let i = 0;
    let j = 0;
    while (i < n && j < m) {
        if (a[i] === b[j]) {
            parts.push({ op: 'same', text: a[i] });
            i++;
            j++;
        } else if (lcs[i + 1][j] >= lcs[i][j + 1]) parts.push({ op: 'del', text: a[i++] });
        else parts.push({ op: 'add', text: b[j++] });
    }
    while (i < n) parts.push({ op: 'del', text: a[i++] });
    while (j < m) parts.push({ op: 'add', text: b[j++] });
    return parts;
}
