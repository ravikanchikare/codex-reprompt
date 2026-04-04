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
//! - `api_utils` — shared SSE parsing utilities for API calls
//! - `config` — `RepromptResult`, `RepromptConfig`, `TaskType`, overlay state types
//! - `insights` — `/reprompt-insights` analysis, storage, and overlay
//! - `overlay` — `RepromptOverlay` Ratatui widget
//! - `profile_config` — profile configuration loading from `~/.codex/reprompt/`
//! - `project_context` — filtered project-structure context generation + caching
//! - `refinement` — async `refine_input()` API call
//! - `relevant_context` — relevant file/tool matching + refined mention resolution

pub(crate) mod api_utils;
pub(crate) mod config;
pub(crate) mod insights;
pub(crate) mod overlay;
pub(crate) mod profile_config;
pub(crate) mod project_context;
pub(crate) mod refinement;
pub(crate) mod relevant_context;
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
pub(crate) use project_context::ProjectContextCache;
pub(crate) use project_context::ProjectContextOptions;
pub(crate) use project_context::ProjectContextSnapshot;
pub(crate) use relevant_context::RelevantAppPrompt;
pub(crate) use relevant_context::RelevantPluginPrompt;
pub(crate) use relevant_context::RelevantSkillPrompt;
pub(crate) use relevant_context::RepromptResolutionContext;
pub(crate) use relevant_context::ResolvedRepromptInput;
pub(crate) use thread_context::ThreadContextBuffer;
