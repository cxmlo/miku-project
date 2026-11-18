//! Native Windows Global System Media Transport Controls (GSMTC) bridge.
//!
//! Queries `Windows.Media.Control.GlobalSystemMediaTransportControlsSessionManager`
//! directly via WinRT COM ABI vtable calls loaded dynamically from `combase.dll`
//! (zero external crates, zero child processes, bounded wait timeouts).

#[cfg(windows)]
mod winrt;

use serde::{Deserialize, Serialize};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GsmtcNowPlaying {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub app_id: String,
    pub playback_status: String,
    pub playing: bool,
    pub duration_sec: u64,
    pub position_sec: u64,
    pub has_session: bool,
    pub source: String,
}

impl GsmtcNowPlaying {
    pub fn fallback(source: &str) -> Self {
        Self {
            title: "Sin música".to_string(),
            artist: "—".to_string(),
            album: String::new(),
            app_id: String::new(),
            playback_status: "None".to_string(),
            playing: false,
            duration_sec: 0,
            position_sec: 0,
            has_session: false,
            source: source.to_string(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GsmtcControlAction {
    TogglePlayPause,
    Play,
    Pause,
    Next,
    Previous,
}

impl GsmtcControlAction {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "play_pause" | "toggle" => Some(Self::TogglePlayPause),
            "play" => Some(Self::Play),
            "pause" => Some(Self::Pause),
            "next" => Some(Self::Next),
            "prev" | "previous" => Some(Self::Previous),
            _ => None,
        }
    }
}

fn clean_media_text(raw: &str, max_chars: usize) -> String {
    raw.chars()
        .filter(|c| !c.is_control())
        .collect::<String>()
        .trim()
        .chars()
        .take(max_chars)
        .collect()
}

pub fn playback_status_label(code: i32) -> &'static str {
    match code {
        0 => "Closed",
        1 => "Opened",
        2 => "Changing",
        3 => "Stopped",
        4 => "Playing",
        5 => "Paused",
        _ => "None",
    }
}

/// Normalizes raw Windows GSMTC fields into a safe UI-ready payload.
#[allow(clippy::too_many_arguments)]
pub fn sanitize_media_snapshot(
    raw_title: &str,
    raw_artist: &str,
    raw_album: &str,
    raw_app_id: &str,
    status_code: i32,
    duration_sec: u64,
    position_sec: u64,
    has_session: bool,
    source: &str,
) -> GsmtcNowPlaying {
    if !has_session {
        return GsmtcNowPlaying::fallback(source);
    }

    let cleaned_title = clean_media_text(raw_title, 180);
    let cleaned_artist = clean_media_text(raw_artist, 140);
    let cleaned_album = clean_media_text(raw_album, 140);
    let cleaned_app = clean_media_text(raw_app_id, 160);

    let playing = status_code == 4;
    let status_label = playback_status_label(status_code).to_string();
    let title = if cleaned_title.is_empty() || cleaned_title == "Sin reproducción" {
        "Sin música".to_string()
    } else {
        cleaned_title
    };
    let artist = if cleaned_artist.is_empty() {
        "—".to_string()
    } else {
        cleaned_artist
    };
    let clamped_pos = if duration_sec > 0 {
        position_sec.min(duration_sec)
    } else {
        position_sec
    };
    let active_session = title != "Sin música" || playing;

    GsmtcNowPlaying {
        title,
        artist,
        album: cleaned_album,
        app_id: cleaned_app,
        playback_status: status_label,
        playing,
        duration_sec,
        position_sec: clamped_pos,
        has_session: active_session,
        source: source.to_string(),
    }
}

static GSMTC_CACHE: OnceLock<Mutex<Option<(Instant, GsmtcNowPlaying)>>> = OnceLock::new();

/// Queries active Windows GSMTC session with a tiny TTL cache (750ms) so UI
/// polling stays snappy and avoids COM churn.
pub fn query_now_playing_cached() -> GsmtcNowPlaying {
    let cache = GSMTC_CACHE.get_or_init(|| Mutex::new(None));
    if let Ok(guard) = cache.lock() {
        if let Some((ts, ref cached)) = *guard {
            if ts.elapsed() < Duration::from_millis(750) {
                return cached.clone();
            }
        }
    }

    let fresh = query_now_playing(180);
    if let Ok(mut guard) = cache.lock() {
        *guard = Some((Instant::now(), fresh.clone()));
    }
    fresh
}

pub fn invalidate_now_playing_cache() {
    if let Some(cache) = GSMTC_CACHE.get() {
        if let Ok(mut guard) = cache.lock() {
            *guard = None;
        }
    }
}

pub fn query_now_playing(timeout_ms: u64) -> GsmtcNowPlaying {
    #[cfg(windows)]
    {
        winrt::query_now_playing_winrt(timeout_ms.clamp(25, 400))
    }
    #[cfg(not(windows))]
    {
        let _ = timeout_ms;
        GsmtcNowPlaying::fallback("fallback")
    }
}

pub fn send_control_action(action: GsmtcControlAction, timeout_ms: u64) -> bool {
    #[cfg(windows)]
    {
        let handled = winrt::send_control_action_winrt(action, timeout_ms.clamp(25, 350));
        if handled {
            invalidate_now_playing_cache();
        }
        handled
    }
    #[cfg(not(windows))]
    {
        let _ = (action, timeout_ms);
        false
    }
}

#[cfg(test)]
mod tests {
    use super::{
        playback_status_label, query_now_playing, sanitize_media_snapshot, GsmtcControlAction,
        GsmtcNowPlaying,
    };

    #[test]
    fn test_8_5_gsmtc_sanitize_and_fallback_contract() {
        let fb = GsmtcNowPlaying::fallback("fallback");
        assert!(!fb.playing);
        assert!(!fb.has_session);
        assert_eq!(fb.title, "Sin música");
        assert_eq!(fb.artist, "—");

        let active = sanitize_media_snapshot(
            "  World is Mine\n",
            "  ryo (supercell) ",
            "supercell",
            "Spotify.exe",
            4,
            255,
            320,
            true,
            "winrt-gsmtc",
        );
        assert!(active.playing);
        assert!(active.has_session);
        assert_eq!(active.title, "World is Mine");
        assert_eq!(active.artist, "ryo (supercell)");
        assert_eq!(active.playback_status, "Playing");
        assert_eq!(active.position_sec, 255);
        assert_eq!(playback_status_label(5), "Paused");

        let empty_title = sanitize_media_snapshot(
            "Sin reproducción",
            "",
            "",
            "msedge.exe",
            5,
            0,
            0,
            true,
            "winrt-gsmtc",
        );
        assert_eq!(empty_title.title, "Sin música");
        assert_eq!(empty_title.artist, "—");
        assert!(!empty_title.playing);
    }

    #[test]
    fn test_8_5_gsmtc_action_parsing_and_safe_live_query() {
        assert_eq!(
            GsmtcControlAction::parse("toggle"),
            Some(GsmtcControlAction::TogglePlayPause)
        );
        assert_eq!(
            GsmtcControlAction::parse("play_pause"),
            Some(GsmtcControlAction::TogglePlayPause)
        );
        assert_eq!(
            GsmtcControlAction::parse("prev"),
            Some(GsmtcControlAction::Previous)
        );
        assert_eq!(GsmtcControlAction::parse("vol_up"), None);

        // Must never panic or hang even when zero media apps are running in CI/test host.
        let snap = query_now_playing(120);
        assert!(!snap.title.is_empty());
        assert!(!snap.artist.is_empty());
        assert!(snap.source == "winrt-gsmtc" || snap.source == "fallback");
    }
}
