//! Desktop-shell experience: boot splash → desktop with warped grid
//! wallpaper, bottom tray, and floating app windows.

pub mod audio;
pub mod boot;
pub mod desktop;
pub mod grid;
pub mod sidebar;
pub mod tray;

use web_time::Instant;

/// Top-level visual phase of the app.
pub enum Phase {
    /// Boot splash showing the WeftOS logo with a short fade sequence.
    Boot { started: Instant, sfx_played: bool },
    /// Live desktop with tray and app windows.
    Desktop,
}

impl Phase {
    pub fn boot() -> Self {
        Phase::Boot {
            started: Instant::now(),
            sfx_played: false,
        }
    }
}

/// Boot timeline (seconds). Bumped so the logo actually has a moment to
/// breathe on fast machines.
pub const BOOT_LEN: f32 = 4.2;
pub const BOOT_FADE_IN: f32 = 0.5;
pub const BOOT_HOLD: f32 = 3.0;
pub const BOOT_FADE_OUT: f32 = 0.7;

/// Derive the opacity of the logo over the boot timeline.
pub fn boot_logo_alpha(elapsed: f32) -> f32 {
    if elapsed < BOOT_FADE_IN {
        elapsed / BOOT_FADE_IN
    } else if elapsed < BOOT_FADE_IN + BOOT_HOLD {
        1.0
    } else {
        let t = (elapsed - BOOT_FADE_IN - BOOT_HOLD) / BOOT_FADE_OUT;
        (1.0 - t).clamp(0.0, 1.0)
    }
}
