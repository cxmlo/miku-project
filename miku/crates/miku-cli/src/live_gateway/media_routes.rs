//! Media & Audio loopback API routes:
//! - `GET /api/audio/devices`: Windows WaveOut output device enumeration
//! - `GET /api/media/now_playing`: real Windows GSMTC session metadata + master volume
//! - `POST /api/media/control`: Gatekeeper-checked GSMTC + VK media/volume transport

use miku_gatekeeper::{Capability, GatekeeperVerdict};
use miku_platform::gsmtc::{
    query_now_playing_cached, send_control_action, GsmtcControlAction, GsmtcNowPlaying,
};
use serde_json::{json, Value};

use super::api_handler::LiveMikuApiHandler;
use super::runtime_state::persist_json;
use super::storage_actor::now_epoch_ms;

pub(super) fn try_dispatch_media_api(
    handler: &LiveMikuApiHandler,
    method: &str,
    path: &str,
    parsed: &Value,
) -> Option<(u16, Value)> {
    match (method, path) {
        ("GET", "/api/audio/devices") => Some(handle_audio_devices()),
        ("GET", "/api/media/now_playing") => Some(handle_now_playing(handler)),
        ("POST", "/api/media/control") => Some(handle_media_control(handler, parsed)),
        _ => None,
    }
}

fn handle_audio_devices() -> (u16, Value) {
    let devices = enumerate_windows_output_devices();
    (200, json!({ "ok": true, "devices": devices }))
}

fn handle_now_playing(handler: &LiveMikuApiHandler) -> (u16, Value) {
    let np: GsmtcNowPlaying = query_now_playing_cached();
    let volume = {
        let st = handler.state.lock().expect("state mutex");
        st.settings
            .get("voice_volume")
            .and_then(|v| v.as_u64().or_else(|| v.as_f64().map(|f| f.round() as u64)))
            .unwrap_or(80)
            .min(100)
    };
    (
        200,
        json!({
            "ok": true,
            "now_playing": {
                "title": np.title,
                "artist": np.artist,
                "album": np.album,
                "app_id": np.app_id,
                "playback_status": np.playback_status,
                "playing": np.playing,
                "duration_sec": np.duration_sec,
                "position_sec": np.position_sec,
                "has_session": np.has_session,
                "source": np.source,
            },
            "volume": volume,
        }),
    )
}

#[cfg(windows)]
fn enumerate_windows_output_devices() -> Vec<Value> {
    #[repr(C)]
    struct WaveOutCapsW {
        w_mid: u16,
        w_pid: u16,
        v_driver_version: u32,
        sz_pname: [u16; 32],
        dw_formats: u32,
        w_channels: u16,
        w_reserved1: u16,
        dw_support: u32,
    }

    #[link(name = "winmm")]
    extern "system" {
        fn waveOutGetNumDevs() -> u32;
        fn waveOutGetDevCapsW(u_device_id: usize, pwoc: *mut WaveOutCapsW, cbwoc: u32) -> u32;
    }

    let mut list = Vec::new();
    let count = unsafe { waveOutGetNumDevs() };
    for idx in 0..count.min(32) {
        let mut caps = std::mem::MaybeUninit::<WaveOutCapsW>::zeroed();
        let rc = unsafe {
            waveOutGetDevCapsW(
                idx as usize,
                caps.as_mut_ptr(),
                std::mem::size_of::<WaveOutCapsW>() as u32,
            )
        };
        if rc == 0 {
            let caps = unsafe { caps.assume_init() };
            let nul = caps.sz_pname.iter().position(|&c| c == 0).unwrap_or(32);
            let name = String::from_utf16_lossy(&caps.sz_pname[..nul])
                .trim()
                .to_string();
            if !name.is_empty() {
                list.push(json!({
                    "id": idx,
                    "name": name,
                    "channels": caps.w_channels,
                    "is_default": idx == 0
                }));
            }
        }
    }
    list
}

#[cfg(not(windows))]
fn enumerate_windows_output_devices() -> Vec<Value> {
    Vec::new()
}

pub(super) fn handle_media_control(handler: &LiveMikuApiHandler, parsed: &Value) -> (u16, Value) {
    let action = parsed
        .get("action")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();

    let vk: u8 = match action {
        "play_pause" | "toggle" | "play" | "pause" => 0xB3,
        "next" => 0xB0,
        "prev" | "previous" => 0xB1,
        "mute" => 0xAD,
        "vol_down" => 0xAE,
        "vol_up" => 0xAF,
        "vol" => 0,
        _ => return (400, json!({ "ok": false, "error": "unsupported media action" })),
    };

    let target = format!("media.{}", action);
    let verdict = {
        let mut st = handler.state.lock().expect("state mutex");
        st.gatekeeper.evaluate_request(
            "miku",
            Capability::Input,
            &target,
            action,
            now_epoch_ms(),
        )
    };
    if let GatekeeperVerdict::Deny { detail, .. } = verdict {
        return (403, json!({ "ok": false, "error": detail }));
    }

    if action == "vol" {
        let vol = parsed
            .get("value")
            .or_else(|| parsed.get("val"))
            .and_then(|v| v.as_u64().or_else(|| v.as_f64().map(|f| f.round().clamp(0.0, 100.0) as u64)))
            .unwrap_or(80)
            .min(100);
        let snap = {
            let mut st = handler.state.lock().expect("state mutex");
            st.settings.insert("voice_volume".to_string(), json!(vol));
            persist_json(&st.data_dir.join("settings.json"), &st.settings);
            st.snapshot_json()
        };
        let _ = handler.events_tx.send(json!({
            "type": "music",
            "data": { "volume": vol }
        }));
        handler.broadcast_state(&snap);
        return (200, json!({ "ok": true, "action": "vol", "volume": vol }));
    }

    let gsmtc_handled = dispatch_transport_or_vk(action, vk);
    let np = query_now_playing_cached();
    let _ = handler.events_tx.send(json!({
        "type": "music",
        "data": {
            "title": np.title,
            "artist": np.artist,
            "playing": np.playing,
            "dur": np.duration_sec,
            "pos": np.position_sec
        }
    }));

    (
        200,
        json!({
            "ok": true,
            "action": action,
            "vk": vk,
            "gsmtc_handled": gsmtc_handled,
            "now_playing": np
        }),
    )
}

#[cfg(not(test))]
fn dispatch_transport_or_vk(action: &str, vk: u8) -> bool {
    if let Some(cmd) = GsmtcControlAction::parse(action) {
        if send_control_action(cmd, 150) {
            return true;
        }
    }
    if vk != 0 {
        dispatch_media_vk(vk);
    }
    false
}

#[cfg(test)]
fn dispatch_transport_or_vk(action: &str, _vk: u8) -> bool {
    let _ = (GsmtcControlAction::parse(action), send_control_action);
    false
}

#[cfg(all(windows, not(test)))]
pub(super) fn dispatch_media_vk(vk: u8) {
    const KEYEVENTF_EXTENDEDKEY: u32 = 0x0001;
    const KEYEVENTF_KEYUP: u32 = 0x0002;
    #[link(name = "user32")]
    extern "system" {
        fn keybd_event(b_vk: u8, b_scan: u8, dw_flags: u32, dw_extra_info: usize);
    }
    unsafe {
        keybd_event(vk, 0, KEYEVENTF_EXTENDEDKEY, 0);
        keybd_event(vk, 0, KEYEVENTF_EXTENDEDKEY | KEYEVENTF_KEYUP, 0);
    }
}

#[cfg(any(not(windows), test))]
pub(super) fn dispatch_media_vk(_vk: u8) {}

#[cfg(test)]
mod tests {
    use super::try_dispatch_media_api;
    use crate::live_gateway::LiveMikuApiHandler;
    use serde_json::json;

    #[test]
    fn test_8_5_media_routes_now_playing_volume_and_kill_switch() {
        let uniq = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let tmp = std::env::temp_dir().join(format!("miku_media_route_{}", uniq));
        let handler = LiveMikuApiHandler::new(&tmp, 0, true).expect("handler");

        let (code, body) = try_dispatch_media_api(&handler, "GET", "/api/media/now_playing", &json!({}))
            .expect("now_playing route");
        assert_eq!(code, 200);
        assert_eq!(body.get("ok").and_then(|v| v.as_bool()), Some(true));
        let np = body.get("now_playing").expect("now_playing object");
        assert!(np.get("title").and_then(|v| v.as_str()).is_some());
        assert!(np.get("artist").and_then(|v| v.as_str()).is_some());
        assert!(np.get("playing").and_then(|v| v.as_bool()).is_some());

        let (code, body) = try_dispatch_media_api(
            &handler,
            "POST",
            "/api/media/control",
            &json!({ "action": "vol", "value": 64 }),
        )
        .expect("vol control route");
        assert_eq!(code, 200);
        assert_eq!(body.get("volume").and_then(|v| v.as_u64()), Some(64));

        let (code, np_after) = try_dispatch_media_api(&handler, "GET", "/api/media/now_playing", &json!({}))
            .expect("now_playing after vol");
        assert_eq!(code, 200);
        assert_eq!(np_after.get("volume").and_then(|v| v.as_u64()), Some(64));

        let (code, _) = try_dispatch_media_api(
            &handler,
            "POST",
            "/api/media/control",
            &json!({ "action": "bogus_op" }),
        )
        .expect("invalid media action");
        assert_eq!(code, 400);

        let _ = handler.set_kill_switch(true);
        let (code, _) = try_dispatch_media_api(
            &handler,
            "POST",
            "/api/media/control",
            &json!({ "action": "toggle" }),
        )
        .expect("kill switch blocks media control");
        assert_eq!(code, 403);

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
