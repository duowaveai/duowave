//! Calls through a LiveKit room: the family page and its tokens (always), the signalling over data messages
//! (always), and the bridge that puts the station in the room (feature `livekit`: LiveKit's SDK with a prebuilt
//! libwebrtc).

#[cfg(feature = "livekit")]
pub mod livekit_bridge;
pub mod livekit_calls;
pub mod page;
pub mod token;

#[cfg(feature = "livekit")]
pub use livekit_bridge::LiveKitBridge;
pub use livekit_calls::{LiveKitCalls, RoomLink, LIVEKIT_TOPIC};
pub use page::livekit_page;
pub use token::{access_link, app_access, app_identity, app_name, room_token, AppAccess, Invite, APP_TOKEN_TTL};
