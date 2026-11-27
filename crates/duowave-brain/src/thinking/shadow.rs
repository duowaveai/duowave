//! Shadow without a second call of the model (docs/plans/thinking.md, §10): beside the live card Thinking builds the
//! card it would have sent, and after the turn checks the tools the model really called.
//!
//! - `hit`: every tool called was on the card — the same rounds with fewer tokens;
//! - `miss`: a tool called was only in a group of the catalog — one round more;
//! - `none`: no tool was called — the card was enough.
//!
//! So the hit rate, the tokens saved and the extra rounds are known before anyone hears the card. `recall` and `open`
//! are on every card, so they never miss.

use super::line::{Line, Shadow};

/// The tools a turn called, in order, without the ones on every card.
pub fn called(line: &Line) -> Vec<String> {
    let mut called: Vec<String> = Vec::new();
    for call in line.rounds.iter().flat_map(|round| round.calls.iter()) {
        if matches!(call.as_str(), "recall" | "open" | "use") || called.contains(call) {
            continue;
        }
        called.push(call.clone());
    }
    called
}

/// How a card would have done on what was called.
pub fn judge(card: &[String], called: &[String]) -> &'static str {
    if called.is_empty() {
        "none"
    } else if called.iter().all(|tool| card.contains(tool)) {
        "hit"
    } else {
        "miss"
    }
}

/// The card's shadow entry for a turn's line.
pub fn card(variant: &str, tokens: u32, tools: Vec<String>, line: &Line) -> Shadow {
    let hit = judge(&tools, &called(line)).to_string();
    Shadow { part: "card".into(), variant: variant.into(), tokens, tools, hit, decisions: Vec::new() }
}

#[cfg(test)]
mod tests {
    use super::super::line::Round;
    use super::*;

    fn line(calls: &[&[&str]]) -> Line {
        Line {
            rounds: calls
                .iter()
                .map(|calls| Round { calls: calls.iter().map(|call| call.to_string()).collect(), ..Round::default() })
                .collect(),
            ..Line::default()
        }
    }

    #[test]
    fn a_card_hits_misses_or_is_not_needed() {
        let card: Vec<String> = ["get_weather", "set_timer", "recall"].map(String::from).to_vec();
        assert_eq!(judge(&card, &called(&line(&[&["get_weather"], &[]]))), "hit");
        assert_eq!(judge(&card, &called(&line(&[&["calendar_events"]]))), "miss");
        assert_eq!(judge(&card, &called(&line(&[&[]]))), "none");
        assert_eq!(judge(&card, &called(&line(&[&["recall"]]))), "none", "recall is on every card");
        assert_eq!(
            called(&line(&[&["get_weather", "get_weather"], &["open", "set_timer"]])),
            ["get_weather", "set_timer"]
        );
    }
}
