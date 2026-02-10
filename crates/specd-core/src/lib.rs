// ABOUTME: Core library for specd, containing domain types, events, and commands.
// ABOUTME: This crate defines the shared data model used across all specd components.

pub mod card;
pub mod model;

pub use card::Card;
pub use model::SpecCore;
