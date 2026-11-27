//! The frames (docs/plans/router-and-works.md, block A, 5): what the gateway cuts off itself, before any policy — no
//! table and no one else's router goes past them. Privacy is the regulator «Что может уходить из дома» of «Дом и
//! помощники» (the setting `away`): a model whose place it keeps in is not offered. There is no second regulator. The
//! budgets are the other frame (budgets.rs): they say the step a work's ask is at.
//!
//! A seat's model of «Мозг» is held to the regulator by «Мозг» itself — the answer goes to words or another model —, so
//! the frame passes it as it is (`Offer::framed`). The models a table names, and the agents of the works in folders,
//! are held here: Claude Code is always someone else's cloud — with «Только дома» or «Дом и моё облако» it is not
//! ready, and says so in words; an agent of a card goes where its model goes: «облако агента» is someone else's cloud,
//! «модель через DuoWave» is let through as the conversation is.

use super::policy::Offer;
use super::Place;
use crate::core::helpers::{Away, Reach};

/// The reach of a place, as the regulator counts it.
pub fn reach_of(place: Place) -> Reach {
    match place {
        Place::Home => Reach::Home,
        Place::Mine => Reach::Mine,
        Place::Cloud => Reach::Foreign,
    }
}

/// Whether the regulator lets an offer through.
pub fn lets(away: Away, offer: &Offer) -> bool {
    offer.framed || away.lets(reach_of(offer.place))
}

/// The offers the regulator lets through, by their places in `offers`.
pub fn let_through(away: Away, offers: &[Offer]) -> Vec<usize> {
    (0..offers.len()).filter(|&at| lets(away, &offers[at])).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn offer(place: Place, framed: bool) -> Offer {
        Offer { provider: "p".into(), model: "m".into(), place, price: None, seat: false, framed }
    }

    #[test]
    fn the_regulator_keeps_the_clouds_in_but_not_what_brain_already_held() {
        let offers = [offer(Place::Home, false), offer(Place::Mine, false), offer(Place::Cloud, false)];
        assert_eq!(let_through(Away::Any, &offers), [0, 1, 2]);
        assert_eq!(let_through(Away::Mine, &offers), [0, 1]);
        assert_eq!(let_through(Away::Home, &offers), [0]);
        assert!(lets(Away::Home, &offer(Place::Cloud, true)));
    }
}
