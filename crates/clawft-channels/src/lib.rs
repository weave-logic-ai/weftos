//! Channel plugin system for clawft.
//!
//! Provides the trait-based plugin architecture for chat channels.
//! Each channel (Telegram, Slack, Discord, etc.) implements the [`Channel`]
//! trait and is registered via a [`ChannelFactory`]. The [`PluginHost`]
//! manages channel lifecycle (registration, start, stop) and routes
//! outbound messages to the appropriate channel.
//!
//! # Architecture
//!
//! ```text
//! ChannelFactory ──build()──> Arc<dyn Channel>
//!                                 │
//!                     PluginHost.init_channel()
//!                                 │
//!                     PluginHost.start_channel()
//!                           │           │
//!                   CancellationToken   Arc<dyn ChannelHost>
//!                           │           │
//!                     Channel::start(host, cancel)
//! ```
//!
//! # Error handling
//!
//! Channel operations return [`ChannelError`](clawft_types::error::ChannelError)
//! from the `clawft-types` crate. This crate re-exports it for convenience.

pub mod discord;
#[cfg(feature = "email")]
pub mod email;
#[cfg(feature = "google-chat")]
pub mod google_chat;
pub mod host;
#[cfg(feature = "irc")]
pub mod irc;
#[cfg(feature = "matrix")]
pub mod matrix;
pub mod plugin_host;
#[cfg(feature = "signal")]
pub mod signal;
pub mod slack;
#[cfg(feature = "teams")]
pub mod teams;
pub mod telegram;
pub mod traits;
#[cfg(feature = "voice")]
pub mod voice;
pub mod web;
#[cfg(feature = "whatsapp")]
pub mod whatsapp;

pub use host::PluginHost;
pub use traits::*;

// Re-export the canonical error type so callers do not need to depend
// on clawft-types directly for channel errors.
pub use clawft_types::error::ChannelError;
