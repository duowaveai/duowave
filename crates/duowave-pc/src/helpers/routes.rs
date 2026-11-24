//! `/api/v1/helpers*` — with the computer's own: its connection key and its clients («Помогаю», `helping`).

use axum::routing::{get, post, put};
use axum::Router;

pub use duowave_home::helpers::routes::*;

use super::helping;

pub fn router<S>() -> Router<S>
where
    S: crate::helpers::RouteNeeds + duowave_home::helpers::RouteNeeds,
{
    duowave_home::helpers::routes::router::<S>()
        .route("/api/v1/helpers/key", get(helping::key))
        .route("/api/v1/helpers/key/rotate", post(helping::rotate))
        .route("/api/v1/helpers/key/outside", put(helping::outside))
        .route("/api/v1/helpers/clients", get(helping::clients))
        .route("/api/v1/helpers/clients/{id}", put(helping::set_gives).delete(helping::remove_client))
}
