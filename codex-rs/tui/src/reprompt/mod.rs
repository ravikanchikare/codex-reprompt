//! `/reprompt` — every-turn prompt refinement for the Codex TUI.
//!
//! This module contains the data types, configuration, overlay state machine,
//! refinement API call, and overlay widget for the `/reprompt` feature.
//!
//! `/reprompt` is a TUI-only input interception layer: it sits between the
//! user's submission and `AppCommand::user_turn()`, refining each prompt before
//! it reaches the Codex App Server. The App Server has zero awareness of
//! reprompt.
//!
//! # Module structure
//!
//! - `profile_config` — profile configuration loading from `~/.codex/reprompt/`
//! - `config` — `RepromptResult`, `RepromptConfig`, `TaskType`, overlay state types
//! - `overlay` — `RepromptOverlay` Ratatui widget
//! - `refinement` — async `refine_input()` API call

pub(crate) mod config;
pub(crate) mod overlay;
pub(crate) mod profile_config;
pub(crate) mod refinement;
pub(crate) mod thread_context;

pub(crate) use config::RepromptAuthInfo;
pub(crate) use config::RepromptConfig;
pub(crate) use config::RepromptOverlayAction;
pub(crate) use config::RepromptOverlayData;
pub(crate) use config::RepromptOverlayState;
pub(crate) use config::RepromptResult;
pub(crate) use config::TaskType;
pub(crate) use overlay::RepromptOverlay;
pub(crate) use profile_config::RepromptProfile;
pub(crate) use thread_context::ThreadContextBuffer;
