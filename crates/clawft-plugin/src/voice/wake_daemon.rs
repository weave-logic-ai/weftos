//! Background daemon for continuous wake word detection.
//!
//! Runs the [`WakeWordDetector`] in a background loop, monitoring
//! audio input for the "Hey Weft" trigger phrase. When detected,
//! activates Talk Mode.
//!
//! Currently a **stub implementation** -- real audio capture and
//! rustpotter integration are deferred to the 0.8.x in-process voice
//! backend (see ADR-053).

use tracing::info;

use crate::error::PluginError;
use crate::traits::CancellationToken;

use super::wake::{WakeWordConfig, WakeWordDetector};

/// Daemon that runs wake word detection in the background.
///
/// When the wake word is detected, the daemon can activate Talk Mode.
/// The daemon runs until cancelled via its [`CancellationToken`].
pub struct WakeDaemon {
    detector: WakeWordDetector,
    active: bool,
}

impl WakeDaemon {
    /// Create a new wake daemon with the given configuration.
    pub fn new(config: WakeWordConfig) -> Result<Self, PluginError> {
        let detector = WakeWordDetector::new(config)?;
        Ok(Self {
            detector,
            active: false,
        })
    }

    /// Run the daemon until cancelled.
    ///
    /// STUB: Logs that the daemon is running and waits for cancellation.
    /// Real implementation will continuously capture audio and feed it
    /// to the wake word detector.
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn run(&mut self, cancel: CancellationToken) -> Result<(), PluginError> {
        info!("wake daemon started (stub)");
        self.active = true;
        self.detector.start();

        // Wait for cancellation.
        cancel.cancelled().await;

        self.detector.stop();
        self.active = false;
        info!("wake daemon stopped");
        Ok(())
    }

    /// Check if the daemon is currently active.
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Get a reference to the underlying detector.
    pub fn detector(&self) -> &WakeWordDetector {
        &self.detector
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wake_daemon_create() {
        let config = WakeWordConfig::default();
        let daemon = WakeDaemon::new(config).unwrap();
        assert!(!daemon.is_active());
    }

    #[test]
    fn wake_daemon_detector_access() {
        let config = WakeWordConfig {
            threshold: 0.3,
            ..Default::default()
        };
        let daemon = WakeDaemon::new(config).unwrap();
        assert!((daemon.detector().config().threshold - 0.3).abs() < f32::EPSILON);
    }

    #[tokio::test]
    async fn wake_daemon_run_and_cancel() {
        let config = WakeWordConfig::default();
        let mut daemon = WakeDaemon::new(config).unwrap();
        let cancel = CancellationToken::new();
        let cancel_clone = cancel.clone();

        let handle = tokio::spawn(async move { daemon.run(cancel_clone).await });

        // Give the daemon a moment to start.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Cancel and verify clean shutdown.
        cancel.cancel();
        let result = handle.await.unwrap();
        assert!(result.is_ok());
    }
}
