//! Texts compared and merged by lines (docs/plans/package-history.md, «Объединение»): our own, no crate — the station
//! counts every hundred kilobytes of the gateway.
//!
//! - `matches`: the lines two texts share, in order — the common start and end first, Myers's shortest edit in between.
//!   The middle's search stops at `MAX_EDITS` edits (its trace grows with them): past that the middle counts as
//!   rewritten whole — a worse picture of the change, never a wrong one.
//! - `lines`: what the panel shows — every line kept, gone or new, the unchanged ones far from a change folded.
//! - `merge3`: the three-way merge (diff3): where only one side changed a stretch of the base, that side's lines; both
//!   the same — them; both differently — a conflict with the three versions of the stretch.

use std::collections::HashMap;

use serde::Serialize;

/// Edits the search goes up to in the middle of two texts.
const MAX_EDITS: usize = 600;
/// Unchanged lines shown around a change.
const CONTEXT: usize = 3;

/// The text's lines; a last newline leaves an empty last line, so joining them by `\n` gives the text back.
pub fn split(text: &str) -> Vec<&str> {
    text.split('\n').collect()
}

/// Myers's shortest edit of `a` into `b` (lines as numbers): the matched pairs, or None past `limit` edits.
fn myers(a: &[u32], b: &[u32], limit: usize) -> Option<Vec<(usize, usize)>> {
    let (n, m) = (a.len() as i64, b.len() as i64);
    let limit = limit.min((n + m) as usize) as i64;
    let offset = limit + 1;
    let at = |k: i64| (k + offset) as usize;
    let mut v = vec![0i64; (2 * limit + 3) as usize];
    let mut trace: Vec<Vec<i64>> = Vec::new();
    let mut end = None;
    'search: for d in 0..=limit {
        trace.push(v.clone());
        let mut k = -d;
        while k <= d {
            let mut x =
                if k == -d || (k != d && v[at(k - 1)] < v[at(k + 1)]) { v[at(k + 1)] } else { v[at(k - 1)] + 1 };
            let mut y = x - k;
            while x < n && y < m && a[x as usize] == b[y as usize] {
                x += 1;
                y += 1;
            }
            v[at(k)] = x;
            if x >= n && y >= m {
                end = Some(d);
                break 'search;
            }
            k += 2;
        }
    }
    end?;
    let mut pairs = Vec::new();
    let (mut x, mut y) = (n, m);
    for (d, v) in trace.iter().enumerate().rev() {
        let d = d as i64;
        let k = x - y;
        let previous = if k == -d || (k != d && v[at(k - 1)] < v[at(k + 1)]) { k + 1 } else { k - 1 };
        let px = v[at(previous)];
        let py = px - previous;
        while x > px && y > py {
            pairs.push(((x - 1) as usize, (y - 1) as usize));
            x -= 1;
            y -= 1;
        }
        if d > 0 {
            (x, y) = (px, py);
        }
    }
    pairs.reverse();
    Some(pairs)
}

/// The lines `a` and `b` share, in order: pairs of their numbers.
pub fn matches<'a>(a: &[&'a str], b: &[&'a str]) -> Vec<(usize, usize)> {
    let start = a.iter().zip(b.iter()).take_while(|(x, y)| x == y).count();
    let end = a[start..].iter().rev().zip(b[start..].iter().rev()).take_while(|(x, y)| x == y).count();
    let (middle_a, middle_b) = (&a[start..a.len() - end], &b[start..b.len() - end]);
    let mut numbers: HashMap<&str, u32> = HashMap::new();
    let mut number_of = |lines: &[&'a str]| -> Vec<u32> {
        lines
            .iter()
            .map(|line| {
                let next = numbers.len() as u32;
                *numbers.entry(line).or_insert(next)
            })
            .collect()
    };
    let a_numbers = number_of(middle_a);
    let b_numbers = number_of(middle_b);
    let mut pairs: Vec<(usize, usize)> = (0..start).map(|i| (i, i)).collect();
    if let Some(middle) = myers(&a_numbers, &b_numbers, MAX_EDITS) {
        pairs.extend(middle.into_iter().map(|(i, j)| (start + i, start + j)));
    }
    pairs.extend((0..end).map(|i| (a.len() - end + i, b.len() - end + i)));
    pairs
}

/// A line as the panel shows a change: kept, gone, new — or a fold of unchanged ones.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Line {
    /// ` ` kept, `-` gone, `+` new, `…` unchanged lines folded (`text` says how many).
    pub op: char,
    pub text: String,
}

/// The change of `a` into `b`, line by line, unchanged stretches far from a change folded.
pub fn lines(a: &str, b: &str) -> Vec<Line> {
    let (a, b) = (split(a), split(b));
    let pairs = matches(&a, &b);
    let mut all: Vec<(char, &str)> = Vec::new();
    let (mut i, mut j) = (0, 0);
    for (pi, pj) in pairs.iter().copied().chain(std::iter::once((a.len(), b.len()))) {
        all.extend(a[i..pi].iter().map(|line| ('-', *line)));
        all.extend(b[j..pj].iter().map(|line| ('+', *line)));
        if pi < a.len() {
            all.push((' ', a[pi]));
        }
        (i, j) = (pi + 1, pj + 1);
    }
    // Near a change: within CONTEXT lines of one.
    let changed: Vec<usize> = all.iter().enumerate().filter(|(_, (op, _))| *op != ' ').map(|(at, _)| at).collect();
    let near = |at: usize| changed.iter().any(|change| change.abs_diff(at) <= CONTEXT);
    let mut shown = Vec::new();
    let mut folded = 0usize;
    for (at, (op, text)) in all.iter().enumerate() {
        if *op == ' ' && !near(at) {
            folded += 1;
            continue;
        }
        if folded > 0 {
            shown.push(Line { op: '…', text: folded.to_string() });
            folded = 0;
        }
        shown.push(Line { op: *op, text: text.to_string() });
    }
    if folded > 0 {
        shown.push(Line { op: '…', text: folded.to_string() });
    }
    shown
}

/// A stretch both sides changed differently: the three versions of it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct Conflict {
    /// The base's line it starts at, from 1.
    pub line: usize,
    pub base: String,
    pub ours: String,
    pub theirs: String,
}

/// For each base line, the line of `other` it matches.
fn matched(base: &[&str], other: &[&str]) -> Vec<Option<usize>> {
    let mut found = vec![None; base.len()];
    for (i, j) in matches(base, other) {
        found[i] = Some(j);
    }
    found
}

/// The three-way merge of texts by lines: the merged text, or the stretches both sides changed differently.
pub fn merge3(base: &str, ours: &str, theirs: &str) -> Result<String, Vec<Conflict>> {
    if ours == theirs || theirs == base {
        return Ok(ours.to_string());
    }
    if ours == base {
        return Ok(theirs.to_string());
    }
    let (b, o, t) = (split(base), split(ours), split(theirs));
    let (to_ours, to_theirs) = (matched(&b, &o), matched(&b, &t));
    let mut merged: Vec<&str> = Vec::new();
    let mut conflicts = Vec::new();
    let (mut i, mut x, mut y) = (0usize, 0usize, 0usize);
    loop {
        // A stretch all three share.
        let mut same = 0;
        while i + same < b.len() && to_ours[i + same] == Some(x + same) && to_theirs[i + same] == Some(y + same) {
            same += 1;
        }
        if same > 0 {
            merged.extend_from_slice(&b[i..i + same]);
            (i, x, y) = (i + same, x + same, y + same);
            continue;
        }
        // The next base line both sides still have, ahead of where each is.
        let next = (i..b.len()).find_map(|at| match (to_ours[at], to_theirs[at]) {
            (Some(ox), Some(ty)) if ox >= x && ty >= y => Some((at, ox, ty)),
            _ => None,
        });
        let (ni, nx, ny) = next.unwrap_or((b.len(), o.len(), t.len()));
        let (base_part, ours_part, theirs_part) = (&b[i..ni], &o[x..nx], &t[y..ny]);
        if ours_part == base_part || ours_part == theirs_part {
            merged.extend_from_slice(theirs_part);
        } else if theirs_part == base_part {
            merged.extend_from_slice(ours_part);
        } else {
            conflicts.push(Conflict {
                line: i + 1,
                base: base_part.join("\n"),
                ours: ours_part.join("\n"),
                theirs: theirs_part.join("\n"),
            });
            merged.extend_from_slice(ours_part);
        }
        if next.is_none() {
            break;
        }
        (i, x, y) = (ni, nx, ny);
    }
    if conflicts.is_empty() {
        Ok(merged.join("\n"))
    } else {
        Err(conflicts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_shared_lines_in_order() {
        let a = split("a\nb\nc\nd\ne");
        let b = split("a\nx\nc\nd\ny\ne");
        assert_eq!(matches(&a, &b), vec![(0, 0), (2, 2), (3, 3), (4, 5)]);
        assert_eq!(matches(&split(""), &split("q")), Vec::<(usize, usize)>::new());
        // Past the limit of edits the middle counts as rewritten: start and end still match.
        let long_a: Vec<String> = (0..2000).map(|n| format!("a{n}")).collect();
        let long_b: Vec<String> = (0..2000).map(|n| format!("b{n}")).collect();
        let mut x: Vec<&str> = vec!["top"];
        x.extend(long_a.iter().map(String::as_str));
        x.push("bottom");
        let mut y: Vec<&str> = vec!["top"];
        y.extend(long_b.iter().map(String::as_str));
        y.push("bottom");
        assert_eq!(matches(&x, &y), vec![(0, 0), (2001, 2001)]);
    }

    #[test]
    fn shows_a_change_with_its_context_and_folds_the_rest() {
        let a: String = (1..=20).map(|n| format!("line {n}\n")).collect();
        let b = a.replace("line 10\n", "line ten\n");
        let shown = lines(&a, &b);
        let ops: String = shown.iter().map(|line| line.op).collect();
        assert_eq!(ops, "…   -+   …");
        assert_eq!(
            (shown[0].text.as_str(), shown[4].text.as_str(), shown[5].text.as_str()),
            ("6", "line 10", "line ten")
        );
    }

    #[test]
    fn merges_changes_in_different_places_and_finds_the_same_place_changed_twice() {
        let base = "one\ntwo\nthree\nfour\nfive\n";
        let ours = "one\nTWO\nthree\nfour\nfive\n";
        let theirs = "one\ntwo\nthree\nfour\nFIVE\nsix\n";
        assert_eq!(merge3(base, ours, theirs).unwrap(), "one\nTWO\nthree\nfour\nFIVE\nsix\n");
        // The same change on both sides is no conflict.
        assert_eq!(merge3(base, ours, ours).unwrap(), ours);
        assert_eq!(merge3(base, base, theirs).unwrap(), theirs);
        // One line changed differently on both sides.
        let conflicts = merge3(base, "one\ndva\nthree\nfour\nfive\n", "one\nzwei\nthree\nfour\nfive\n").unwrap_err();
        assert_eq!(
            conflicts,
            vec![Conflict { line: 2, base: "two".into(), ours: "dva".into(), theirs: "zwei".into() }]
        );
        // Lines added at the same place by both: a conflict too, the order is not ours to guess.
        assert!(merge3("a\nb\n", "a\nx\nb\n", "a\ny\nb\n").is_err());
        // One side removes a line the other keeps.
        assert_eq!(merge3("a\nb\nc\n", "a\nc\n", "a\nb\nc\nd\n").unwrap(), "a\nc\nd\n");
    }
}
