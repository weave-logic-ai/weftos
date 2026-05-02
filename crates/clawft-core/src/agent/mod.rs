//! Agent subsystem: loop, context, memory, skills, agent definitions, sandbox.

pub mod agents;
pub mod context;
pub mod context_router;
pub mod cost_budget;
pub mod effects;
pub mod gate;
pub mod helpers;
pub mod identity;
pub mod loop_core;
pub mod memory;
pub mod runtime;
pub mod sandbox;
pub mod sink;
#[cfg(feature = "native")]
pub mod skill_watcher;
pub mod skill_autogen;
pub mod skills;
pub mod skills_v2;
pub mod system_prompt;
pub mod verification;
