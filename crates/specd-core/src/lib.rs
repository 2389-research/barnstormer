// ABOUTME: Core library for specd, containing domain types, events, and commands.
// ABOUTME: This crate defines the shared data model used across all specd components.

pub mod actor;
pub mod card;
pub mod command;
pub mod event;
pub mod model;
pub mod state;
pub mod transcript;

pub use actor::{ActorError, SpecActorHandle, spawn};
pub use card::Card;
pub use command::Command;
pub use event::{Event, EventPayload};
pub use model::SpecCore;
pub use state::{SpecState, UndoEntry};
pub use transcript::{TranscriptMessage, UserQuestion};
