//! Our own skills and the router that picks who serves a request: the capability catalog, the quick rules and the
//! route table, the model's tools, the station's commands (ordinary and typed), our implementations (own/, media/,
//! lookups/, smart_home/) and the service that holds them.

pub mod callbacks;
pub mod capabilities;
pub mod directives;
pub mod display;
pub mod js;
pub mod lookups;
pub mod media;
pub mod own;
pub mod player_memory;
pub mod quick_commands;
pub mod router;
pub mod routes;
pub mod skill;
pub mod skill_stats;
pub mod skills_service;
pub mod smart_home;
pub mod text;
pub mod tools;
pub mod typed_directives;

pub use callbacks::OUR_CALLBACK_PREFIX;
pub use router::{Continuation, Decision, Resolution, RouteInput, Router, RESTRICTED};
pub use skill::{ExecutionContext, Outcome, OwnSkill, PhraseStream};
pub use skills_service::{CallsLinks, DialogAccess, NoConnectors, SkillConnectors, SkillsDeps, SkillsService};
