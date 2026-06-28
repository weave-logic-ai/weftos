//! Services for the clawft framework.
//!
//! Provides cron scheduling, heartbeat monitoring, and MCP client
//! functionality. Each service generates [`InboundMessage`](clawft_types::event::InboundMessage)
//! events that feed into the main message bus.

#[cfg(feature = "api")]
pub mod api;
pub mod clawhub;
pub mod cron_service;
#[cfg(feature = "delegate")]
pub mod delegation;
pub mod error;
pub mod heartbeat;
pub mod mcp;
#[cfg(feature = "rvf")]
pub mod rvf_tools;
