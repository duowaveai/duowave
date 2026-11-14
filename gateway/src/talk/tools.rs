//! The brain's tools in a talk without a station: what the station's conversation offers, without its commands. The
//! gateway's own lookups (the weather, the news, the smart home) go the road the station's do — the tool's intent,
//! the router, the skill — only Yandex is never there; the MCP servers are the ones open to the voice (the
//! microphone hears the room); the connectors give the apps' and the memory's tools. «Сейчас посмотрю.» after a
//! second of waiting, three rounds, and after a text from outside the house only the tools that change nothing.

use std::sync::Arc;

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::origin::COMPUTER;
use super::{Brain, TalkEvent};
use crate::app::App;
use crate::core::connectors::ContributedOffer;
use crate::core::identity::Principal;
use crate::core::requests::{Place, RequestHandle, RequestPhase};
use crate::dialog::llm::{LlmTool, ToolCall};
use crate::dialog::{ToolBox, ToolOutcome, LOOKUP_FILLER};
use crate::mcp::{McpOffer, OfferOptions};
use crate::skills::capabilities::{is_capability, CapabilityName};
use crate::skills::router::{Resolution, RESTRICTED};
use crate::skills::skill::ExecutionContext;
use crate::skills::tools::{tools_for, ToolsKind, SAFE_AFTER_OUTSIDE};

/// What the model reads when a lookup has no way here: the station is not there, and Yandex is never asked.
const NOT_HERE: &str =
    "Not available on this computer yet: tell the user in one short sentence that you cannot do this here yet.";
const NOT_ALLOWED: &str =
    "Not allowed for this person: tell the user in one short sentence that this is not permitted.";

/// What one talk's tools need: the gateway, who speaks, the phrase, the request's trip and the talk's events.
pub(super) struct Deps {
    pub app: Arc<App>,
    pub principal: Principal,
    pub text: String,
    pub trip: Arc<RequestHandle>,
    pub events: mpsc::UnboundedSender<TalkEvent>,
    pub brain: Brain,
}

/// Whether a capability's route lets our side serve it: a server or an app bound to one is offered only then.
fn ours(app: &App) -> impl Fn(CapabilityName) -> bool {
    let routes = app.skills.router.routes().describe();
    move |capability| routes.get(&capability).is_some_and(|route| route.split(',').any(|step| step == "own"))
}

/// The talk's toolbox; None when the model takes no tools or there is nothing to offer.
pub(super) fn toolbox(deps: Deps) -> Option<ToolBox> {
    let app = &deps.app;
    if !app.dialog.tools_enabled() {
        return None;
    }
    let allowed = ours(app);
    let mcp = app.mcp.offer(&deps.principal, OfferOptions { voice: true, capability_allowed: Some(&allowed) });
    let mcp = mcp.map(Arc::new);
    let mut tools = tools_for(ToolsKind::Conversation, Some(&deps.principal));
    if let Some(mcp) = &mcp {
        tools.extend(mcp.tools.iter().cloned());
    }
    let contributions = app
        .connectors
        .tools(&deps.principal)
        .into_iter()
        .filter(|contribution| {
            contribution.permission.strip_prefix("use:").and_then(is_capability).is_none_or(&allowed)
        })
        .collect();
    let contributed = ContributedOffer::new(contributions, &tools).map(Arc::new);
    if let Some(contributed) = &contributed {
        tools.extend(contributed.tools.iter().cloned());
    }
    if tools.is_empty() {
        return None;
    }
    let deps = Arc::new(deps);
    let (run_mcp, run_contributed) = (mcp.clone(), contributed.clone());
    let mut toolbox = ToolBox::new(tools, move |call: ToolCall, cancel: CancellationToken| {
        let (deps, mcp, contributed) = (deps.clone(), run_mcp.clone(), run_contributed.clone());
        async move { run(&deps, mcp, contributed, call, cancel).await }
    });
    toolbox.filler = Some(LOOKUP_FILLER);
    if let Some(mcp) = &mcp {
        toolbox.rounds = Some(3);
        toolbox.instructions = Some(mcp.instructions.clone());
    } else if contributed.is_some() {
        toolbox.rounds = Some(3);
    }
    toolbox.safe_after_taint = Some(Arc::new(move |offered: &[LlmTool]| {
        offered
            .iter()
            .filter(|tool| {
                SAFE_AFTER_OUTSIDE.contains(&tool.name.as_str())
                    || mcp.as_ref().is_some_and(|mcp| mcp.owns(&tool.name) && mcp.safe(&tool.name))
                    || contributed.as_ref().is_some_and(|offer| offer.owns(&tool.name) && offer.safe(&tool.name))
            })
            .cloned()
            .collect()
    }));
    Some(toolbox)
}

/// One call of the model's: the trip and the talk's events show which tool works — never its arguments — and the
/// brain thinks again once it returned.
async fn run(
    deps: &Deps,
    mcp: Option<Arc<McpOffer>>,
    contributed: Option<Arc<ContributedOffer>>,
    call: ToolCall,
    cancel: CancellationToken,
) -> anyhow::Result<ToolOutcome> {
    let working = |by: Place, name: &str| {
        deps.trip.step(RequestPhase::Tool, Some(by), Some(name));
        let _ = deps.events.send(TalkEvent::Tool { name: name.to_string() });
    };
    let thinking = || deps.trip.step(RequestPhase::Think, Some(deps.brain.by), Some(&deps.brain.what));
    if let Some(offer) = contributed.filter(|offer| offer.owns(&call.name)) {
        working(Place::Own, &call.name);
        // No station answers here: a tool that plays or speaks through one has nowhere to send its commands.
        let outcome = offer.run(&call, &deps.principal, cancel).await;
        thinking();
        return Ok(outcome);
    }
    if let Some(mcp) = mcp.filter(|mcp| mcp.owns(&call.name)) {
        let server = mcp.server_of(&call.name);
        let by = if server.as_ref().is_some_and(|server| server.cloud) { Place::Cloud } else { Place::Own };
        working(by, server.as_ref().map_or(call.name.as_str(), |server| server.title.as_str()));
        let outcome = mcp.run(&call, &cancel).await.map_err(|cancelled| anyhow::anyhow!("{cancelled}"))?;
        thinking();
        return Ok(outcome);
    }
    let intent = match deps.app.skills.intent_of(&call) {
        Ok(intent) => intent,
        Err(error) => return Ok(ToolOutcome::error(format!("error: {error}"))),
    };
    let info = intent.capability.info();
    if info.station {
        return Ok(ToolOutcome::from(NOT_HERE));
    }
    // Yandex is never there for a talk without a station: its route says «not here yet».
    let skill = match deps.app.skills.router.resolve(&intent, false, Some(&deps.principal)) {
        Resolution::Own(skill) => skill,
        Resolution::Unsupported { reason } if reason == RESTRICTED => return Ok(ToolOutcome::from(NOT_ALLOWED)),
        Resolution::Unsupported { .. } | Resolution::Yandex => return Ok(ToolOutcome::from(NOT_HERE)),
    };
    working(Place::Own, info.title);
    let context = ExecutionContext {
        text: deps.text.clone(),
        cancel,
        speaker: Some(deps.principal.clone()),
        ..ExecutionContext::new(COMPUTER, chrono::Utc::now())
    };
    let outcome = skill.execute(&intent, &context).await?;
    thinking();
    // The skill's commands for a station (the weather on its display) have nowhere to go here: its words do.
    let content = outcome.data.or(outcome.speech).unwrap_or_else(|| "done".into());
    // Headlines are other people's text: after them the model is offered nothing that acts.
    Ok(if intent.capability == CapabilityName::News {
        ToolOutcome::tainted(content)
    } else {
        ToolOutcome::from(content)
    })
}
