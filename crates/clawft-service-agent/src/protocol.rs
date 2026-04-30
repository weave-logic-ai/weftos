//! Wire-format types for `agent.chat`.
//!
//! Since WEFT-498 the canonical definitions live in
//! [`clawft_types::agent_chat`]. This module re-exports them so
//! existing imports
//! (`clawft_service_agent::AgentChatParams`,
//! `clawft_service_agent::protocol::AgentChatResult`) keep working
//! without forcing every consumer to follow the new path.
//!
//! The pre-WEFT-498 `From` bridge between the duplicated types in
//! `clawft-weave::protocol` and this module collapsed to identity once
//! the canonical definitions moved upstream.

pub use clawft_types::agent_chat::{
    AgentChatMessage, AgentChatParams, AgentChatResult, AgentChatToolCall, default_conv_id,
};
