//! What «Мозг» needs of the helpers (docs/plans/home-and-helpers.md, block 1, stage 4): which of them give a model, in
//! the household's order, whether each is on the line, and what the regulator «Что может уходить из дома» lets out.
//! The helpers give it (helpers/models.rs). «Мозг» reads it before it takes its own lock, and the helpers never call
//! «Мозг»: the two locks are never held the other way round.

pub use crate::core::helpers::{HelperKind, HelperState, Reach};

/// A helper that gives a model, as «Мозг» sees it at one moment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelHelper {
    /// The entry's id; `self` for each model server of this computer (stage 6).
    pub id: String,
    /// For people: «Компьютер дома».
    pub name: String,
    pub kind: HelperKind,
    /// Where its model is asked now: `{base}/home/v1/llm`, or the server's own address (stage 6).
    pub base_url: String,
    /// What its model's client sends now: the entry's key, or a linked entry's bridge token (stage 7).
    pub key: Option<String>,
    /// How far the road its model is asked by goes now.
    pub reach: Reach,
    /// On the line, and its model ready: what `pick(model)` would take.
    pub up: bool,
    pub state: HelperState,
    /// The later entry of the same helper (the same `helperId`) its model is asked through now, while this one is not
    /// on the line: the road, the reach and the state above are that entry's, and the row stays this one's as stored.
    pub via: Option<String>,
    /// Its contract was read: it declares a model for certain, not only may. A helper not read yet keeps the row it
    /// has, and gets none until its first look says.
    pub read: bool,
    /// Reached through the link's local bridge (stage 7): its address and key are this start's, so its row stores
    /// neither.
    pub linked: bool,
    /// The «Мозг» row that already is this model (a model server of this computer, stage 6): it gets no row of its own.
    pub provider: Option<String>,
    /// This computer's own (stage 6): its words say «на этом компьютере».
    pub builtin: bool,
}

/// The helpers as «Мозг» reads them.
pub trait ModelHelpers: Send + Sync {
    /// Enabled helpers that declare a model, or are not read yet, not switched off for it — in the household's order,
    /// the regulator aside; one helper reached by two entries is one, under the earlier entry (`via`).
    fn model_helpers(&self) -> Vec<ModelHelper>;
    /// Whether the regulator lets a question go this far.
    fn lets(&self, reach: Reach) -> bool;
}
