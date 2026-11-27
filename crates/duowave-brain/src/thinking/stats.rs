//! The numbers for decisions (docs/plans/thinking.md, §10, «Numbers for decisions»): over the turns' lines of a time,
//! a device, a variant — the medians of the first word, of the rounds and of the card's tokens, the cache's share, the
//! card's shadow hit rate and its misses per hundred turns, how often people cut, repeat or say «не то», and the waits.
//! `GET /api/v1/thinking/stats` gives them (routes.rs); the medians of a model's rounds are the rounds policy's
//! expected times too (rounds.rs).

use std::collections::BTreeMap;

use serde::Serialize;

use super::line::Line;

/// Which lines to count.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Query {
    /// Ms since the epoch.
    pub from: Option<u64>,
    pub to: Option<u64>,
    pub device: Option<String>,
    /// A card variant (`all`, `card`, `card_sense`) or a rounds policy (`today`, `by_time`).
    pub variant: Option<String>,
    /// `voice` or `text`.
    pub mode: Option<String>,
}

/// The numbers of the lines counted.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Stats {
    pub turns: usize,
    /// Turns the brain answered (the others: the fast route).
    pub brain: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_word_ms: Option<u64>,
    /// Rounds of the model per turn the brain answered.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rounds: Option<u64>,
    /// The first round's request, in tokens (estimated).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub card_tokens: Option<u64>,
    /// Of the tokens sent, read from the cache.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_share: Option<f64>,
    /// The card in shadow: every called tool on it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hit_rate: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub misses_per_100: Option<f64>,
    /// Of the turns whose next is known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cut_share: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repeated_share: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub not_that_share: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub waits_per_100: Option<f64>,
    /// How many turns each fast step took.
    pub fast: BTreeMap<String, usize>,
    /// The tools called, the most first.
    pub tools: Vec<(String, usize)>,
    /// The turns by the settings' mark: numbers before and after a change are not mixed.
    pub settings: BTreeMap<String, usize>,
}

/// The middle of the values: the lower one of two.
pub fn median(values: &mut [u64]) -> Option<u64> {
    if values.is_empty() {
        return None;
    }
    values.sort_unstable();
    Some(values[(values.len() - 1) / 2])
}

fn share(part: usize, of: usize) -> Option<f64> {
    (of > 0).then(|| (part as f64 / of as f64 * 1000.0).round() / 1000.0)
}

fn per_100(part: usize, of: usize) -> Option<f64> {
    (of > 0).then(|| (part as f64 * 100.0 / of as f64 * 10.0).round() / 10.0)
}

/// Whether a line is one the query counts.
pub fn counts(line: &Line, query: &Query) -> bool {
    let variant =
        query.variant.as_deref().is_none_or(|variant| line.variants.card == variant || line.variants.rounds == variant);
    let mode = query.mode.as_deref().is_none_or(|mode| (mode == "voice") == line.voice);
    variant && mode
}

/// The numbers of these lines.
pub fn of(lines: &[Line]) -> Stats {
    let mut stats = Stats { turns: lines.len(), ..Stats::default() };
    let (mut first, mut rounds, mut card) = (Vec::new(), Vec::new(), Vec::new());
    let (mut input, mut cached) = (0u64, 0u64);
    let (mut hits, mut misses, mut shadowed) = (0usize, 0usize, 0usize);
    let (mut known, mut cut, mut repeated, mut not_that) = (0usize, 0usize, 0usize, 0usize);
    let mut waits = 0usize;
    let mut tools: BTreeMap<String, usize> = BTreeMap::new();
    for line in lines {
        *stats.settings.entry(line.settings.clone()).or_default() += 1;
        if let Some(step) = &line.fast {
            *stats.fast.entry(step.clone()).or_default() += 1;
        }
        if let Some(ms) = line.first_word_ms {
            first.push(ms);
        }
        if !line.rounds.is_empty() {
            stats.brain += 1;
            rounds.push(line.rounds.len() as u64);
        }
        if let Some(parts) = line.card.as_ref().map(|card| card.tokens) {
            card.push((parts.tools + parts.stable + parts.changing + parts.history) as u64);
        }
        for round in &line.rounds {
            input += round.input + round.cache_read;
            cached += round.cache_read;
            for call in &round.calls {
                *tools.entry(call.clone()).or_default() += 1;
            }
        }
        for shadow in line.shadow.iter().filter(|shadow| shadow.part == "card") {
            shadowed += 1;
            match shadow.hit.as_str() {
                "hit" => hits += 1,
                "miss" => misses += 1,
                _ => {}
            }
        }
        if let Some(next) = &line.next {
            known += 1;
            match next.what.as_str() {
                "cut" => cut += 1,
                "repeated" => repeated += 1,
                "not_that" => not_that += 1,
                _ => {}
            }
        }
        waits += line.waits.len();
    }
    stats.first_word_ms = median(&mut first);
    stats.rounds = median(&mut rounds);
    stats.card_tokens = median(&mut card);
    stats.cache_share = (input > 0).then(|| (cached as f64 / input as f64 * 1000.0).round() / 1000.0);
    stats.hit_rate = share(hits, hits + misses);
    stats.misses_per_100 = per_100(misses, shadowed);
    stats.cut_share = share(cut, known);
    stats.repeated_share = share(repeated, known);
    stats.not_that_share = share(not_that, known);
    stats.waits_per_100 = per_100(waits, lines.len());
    let mut tools: Vec<(String, usize)> = tools.into_iter().collect();
    tools.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    tools.truncate(20);
    stats.tools = tools;
    stats
}

#[cfg(test)]
mod tests {
    use super::super::line::{Card, Next, Round, Shadow, Tokens};
    use super::*;

    fn line(first: u64, rounds: &[(u64, u64, &[&str])], hit: &str, next: &str) -> Line {
        Line {
            settings: "s-1".into(),
            voice: true,
            first_word_ms: Some(first),
            card: Some(Card {
                tokens: Tokens { tools: 2000, stable: 1000, changing: 100, history: 400 },
                ..Card::default()
            }),
            rounds: rounds
                .iter()
                .map(|(input, cache, calls)| Round {
                    input: *input,
                    cache_read: *cache,
                    calls: calls.iter().map(|call| call.to_string()).collect(),
                    ..Round::default()
                })
                .collect(),
            shadow: vec![Shadow { part: "card".into(), variant: "card".into(), hit: hit.into(), ..Shadow::default() }],
            next: Some(Next { what: next.into(), ..Next::default() }),
            ..Line::default()
        }
    }

    #[test]
    fn a_scripted_day_gives_its_medians_and_shares() {
        let day = [
            line(900, &[(1000, 3000, &[])], "none", "quiet"),
            line(1400, &[(500, 3500, &["get_weather"]), (600, 3500, &[])], "hit", "quiet"),
            line(2600, &[(800, 3000, &["calendar_events"]), (700, 3200, &[])], "miss", "cut"),
            line(1200, &[(900, 3100, &[])], "none", "repeated"),
        ];
        let stats = of(&day);
        assert_eq!((stats.turns, stats.brain), (4, 4));
        assert_eq!(stats.first_word_ms, Some(1200));
        assert_eq!(stats.rounds, Some(1), "the lower middle of 1, 1, 2, 2");
        assert_eq!(stats.card_tokens, Some(3500));
        assert_eq!(stats.hit_rate, Some(0.5));
        assert_eq!(stats.misses_per_100, Some(25.0));
        assert_eq!((stats.cut_share, stats.repeated_share), (Some(0.25), Some(0.25)));
        let (input, cached) = (4_500u64 + 19_300, 19_300u64);
        assert_eq!(stats.cache_share, Some((cached as f64 / input as f64 * 1000.0).round() / 1000.0));
        assert_eq!(stats.tools, [("calendar_events".to_string(), 1), ("get_weather".to_string(), 1)]);
        assert_eq!(of(&[]).first_word_ms, None);
    }
}
