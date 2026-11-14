//! The gateway's layer of capabilities (docs/plugins/AUTHORING.md, «Слой возможностей шлюза»): each capability is a
//! service on the bus with providers behind it that the owner can swap — models (`system/brain`), voices
//! (`system/voice`), hearing (`system/hearing`), publishing (`system/publish`), media. An app calls the service and
//! never a provider: which provider answers is the gateway's choice by the owner's settings, the providers' keys stay
//! in the gateway's safe, and a new provider is a module here, not a change in any app.

pub mod hearing;
pub mod publish;
pub mod routes;
pub mod voices;
