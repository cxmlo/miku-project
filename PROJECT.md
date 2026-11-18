# Project: Miku Desktop & Android Link Full Orchestration

## Architecture
Miku is a lightweight local autonomous agent orchestrator with a Dynamic Island Win32 interface, WebView2 compositor, `miku-link v1` protocol, fail-closed security (`miku-gatekeeper`), Android companion app, and Plan Cockpit web monitoring.

```
┌─────────────────────────────────────────────────────────────┐
│                       Plan Cockpit                          │
│               (React/TypeScript Vite App)                   │
└──────────────────────────────┬──────────────────────────────┘
                               │ HTTP / WebSocket (Port 47400)
┌──────────────────────────────▼──────────────────────────────┐
│                    miku.exe (Desktop Daemon)                 │
│  ┌───────────────────────┐       ┌───────────────────────┐  │
│  │ miku-ui (Island UI)   │       │ miku-platform         │  │
│  │ - WebView2 Compositor │       │ - PC Control          │  │
│  │ - Mascot 2D Canvas    │       │ - Multi-DPI & COM UIA │  │
│  │ - Vision Capture      │       │ - RAII KeyCombo       │  │
│  │ - Voice STT & Ducking │       │ - Win32 Tray Host     │  │
│  └───────────┬───────────┘       └───────────┬───────────┘  │
│              │                               │              │
│              └───────────────┬───────────────┘              │
│                              │                              │
│                    ┌─────────▼─────────┐                    │
│                    │   miku-protocol   │                    │
│                    │  CaptureTransform │                    │
│                    └───────────────────┘                    │
└──────────────────────────────▲──────────────────────────────┘
                               │ miku-link v1 WebSocket
┌──────────────────────────────┴──────────────────────────────┐
│                 Android Miku-Link Client                    │
│          (Jetpack Compose Adaptive Two-Panel UI)            │
└─────────────────────────────────────────────────────────────┘
```

## Feature Inventory
| # | Feature | Description | Milestone | Source |
|---|---------|-------------|-----------|--------|
| 1 | Gate 0 Frozen Contracts | Settings keys (`voice_stt_enabled`, `voice_stt_engine`, `voice_auto_send`), WS events (`agent_state`, `voice_transcript`), and `CaptureTransform` in `miku-protocol` | M1, M2, M3 | R1 |
| 2 | Mascot Turn Lifecycle | `finalizeTurn(opts)` with 30s watchdog, immediate `runtime.thk.on = 0`, resetting mascot state on all turn exit paths | M1 | R2 |
| 3 | Universal Chat Welcome Chips | Replace Discord/Pinterest prompt chips with app-neutral options ("Estado PC", "Ayuda y comandos", "Buscar archivos") | M1 | R2 |
| 4 | Screen Vision & Window Affinity | `SetWindowDisplayAffinity(hwnd, 0x11)` on transparent frame and native window | M2 | R2 |
| 5 | Vision RAM Compression & Zeroize | Max dimension `<= 1024 px`, output `<= 250 KB`, zeroize memory on turn finish, multimodal LLM payload & local Windows OCR fallback | M2 | R2 |
| 6 | Hybrid Opt-In Voice STT | WebView2 `PermissionRequested` for microphone, WebSpeech fallback to native SAPI / WinRT or Groq Whisper PTT | M2 | R2 |
| 7 | Multi-DPI Coordinate Conversion | Screen pixel coordinates mapped to virtual desktop coordinates via `CaptureTransform` | M3 | R2 |
| 8 | COM UI Automation Safety | COM UI Automation executed in `spawn_blocking` protected by 600ms timeout | M3 | R2 |
| 9 | RAII KeyCombo Guard | `KeyComboGuard` with `Drop` calling `KEYEVENTF_KEYUP` for modifier keys on finish or kill-switch abort | M3 | R2 |
| 10 | Dynamic System Tray Hotkey | `sync_ptt_hotkey(&mut self, enabled: bool)` registering `Alt+Space` only when `voice_stt_enabled == true` | M3 | R1, R2 |
| 11 | Android Adaptive Two-Panel UI | Adaptive layout with `NavigationBar` (< 600dp) vs `NavigationRail` + two-panel (>= 600dp) | M4 | R3 |
| 12 | Android Edge-to-Edge & Voice | `enableEdgeToEdge()`, `imePadding()`, `navigationBarsPadding()`, and `RecognizerIntent` STT | M4 | R3 |
| 13 | Android History & Biometrics | Persistent history, biometric decision approval adhering to `pc_only` rules | M4 | R3 |
| 14 | Android Release Build | `gradlew` wrapper and signed test APK generation ready for device installation | M4 | R3 |
| 15 | Autonomous Integration Verification | `cargo test --workspace -- --test-threads=1` and `cargo clippy --all-targets -- -D warnings` zero errors/warnings | M5 | R4 |
| 16 | Multi-Viewport Visual & DOM Tests | Playwright verification across `1440x900`, `820x900`, `480x844`, `844x390` with Win32 `PrintWindow` on `miku.exe` | M5 | R4 |
| 17 | Plan Cockpit Updates | Live synchronization of milestones, gates, and screenshots in `src/` and `public/captures/*.png` | M5 | R4 |

## Milestones
| # | Name | Scope | Dependencies | Status |
|---|------|-------|-------------|--------|
| M1 | Front 1: Frontend Web & Mascot Lifecycle | `mascot.js`, `app.js`, `state.js`, `scenes/chat.js`, `scenes/settings.js`, `settings_app.js`: `finalizeTurn`, 30s watchdog, `thk.on = 0`, neutral welcome chips, frozen settings keys, WS event handling | None | DONE |
| M2 | Front 2: Rust Vision & Voice Integration | `miku-protocol` (`CaptureTransform`), `miku-ui::vision` (`SetWindowDisplayAffinity`, 1024px/250KB compression, zeroize, multimodal LLM payload, OCR fallback), `miku-ui::webview` (permission handler), `miku-ui::voice` & gateway STT routes | None | DONE |
| M3 | Front 3: Rust PC Control & Tools | `miku-platform::pc_control` (`win32_uia.rs` with 600ms timeout, `win32_input.rs` RAII `KeyComboGuard`, multi-DPI conversion with `CaptureTransform`, < 600 line modular budget), `win32_tray.rs` hotkey sync | None | DONE |
| M4 | Front 4: Android Miku-Link Adaptive UI & Build | `android/app/src/main/kotlin/dev/miku/link/`: Adaptive layouts, edge-to-edge insets, `RecognizerIntent`, biometric approval with `pc_only` enforcement, signed test APK build | None | DONE |
| M5 | Verification Gates & Plan Cockpit Updates | Full workspace single-thread test suite, clippy -D warnings, Playwright multi-viewport visual/DOM checks, and Plan Cockpit UI/screenshot synchronization | M1, M2, M3, M4 | PLANNED |

## Interface Contracts

### 1. Shared `CaptureTransform` Struct (`miku-protocol`)
```rust
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CaptureTransform {
    pub origin_virtual_x: i32,
    pub origin_virtual_y: i32,
    pub width_px: u32,
    pub height_px: u32,
    pub dpi_scale: f32,
}
```
Accessible by `miku-ui::vision`, `miku-platform::pc_control`, and `agent_tool_exec.rs` through `miku_protocol::CaptureTransform`.

### 2. Persistent Settings Keys (Gate 0 Frozen Contract)
```typescript
interface SettingsContracts {
  voice_stt_enabled: boolean; // default: false
  voice_stt_engine: "off" | "browser_webspeech" | "groq_whisper" | "local_sapi"; // default: "off"
  voice_auto_send: boolean; // default: false
}
```
Available in `state.js` `CFG`, settings UIs, and gateway state defaults (`extended_routes.rs:510`).

### 3. Gateway WebSocket Events (Gate 0 Frozen Contract)
- `agent_state`: `{"type": "agent_state", "state": "idle" | "listening" | "thinking" | "working" | "error"}`
- `voice_transcript`: `{"type": "voice_transcript", "text": string, "is_final": boolean, "engine": string}`

### 4. Mascot & Turn Finalizer Contract
- `finalizeTurn(opts: { reason: 'reply' | 'cancel' | 'error' | 'disconnect' | 'watchdog_timeout', error?: boolean, wasBusy?: boolean })`:
  - Clears active watchdog timer.
  - Sets `runtime.thk.on = 0`.
  - Calculates `runtime.thk.dur`.
  - Sets `ch.busy = false`.
  - Resets `MikuMascot` state to `idle` or triggers appropriate emote.
  - Updates chat history and re-renders scene cleanly.

### 5. Win32 Island Window Affinity
- `SetWindowDisplayAffinity(hwnd, 0x0000_0011)` (`WDA_EXCLUDEFROMCAPTURE`) invoked on HWND immediately after creation in `transparent_frame.rs` and `native_window.rs`.

### 6. RAII KeyCombo Modifier Release
- `KeyComboGuard`: tracks depressed virtual keys; in `Drop`, dispatches `KEYEVENTF_KEYUP` for each pressed modifier in reverse order, ensuring modifiers are never stuck on abort or error.

## Code Layout & Exclusive Boundaries

| Agent Front | Exclusive Owned Files | Forbidden Paths |
|---|---|---|
| **Agent 1 (Frontend Web)** | `miku/assets/ui/island/app.js`<br>`miku/assets/ui/island/scenes/chat.js`<br>`miku/assets/ui/island/scenes/settings.js`<br>`miku/assets/ui/shared/core/state.js`<br>`miku/assets/ui/shared/mascot/mascot.js`<br>`miku/assets/ui/settings/settings_app.js` | Any file in `miku/crates/` or `android/` |
| **Agent 2 (Rust Vision & Voice)** | `miku/crates/miku-protocol/src/lib.rs`<br>`miku/crates/miku-ui/src/vision/**`<br>`miku/crates/miku-ui/src/voice/**`<br>`miku/crates/miku-ui/src/webview/**`<br>`miku/crates/miku-ui/src/win32/native_window.rs` | Any file in `miku/assets/ui/` or `miku/crates/miku-platform/` |
| **Agent 3 (Rust PC Control & Tools)** | `miku/crates/miku-platform/src/pc_control/**`<br>`miku/crates/miku-cli/src/win32_tray.rs`<br>`miku/crates/miku-cli/src/desktop_daemon.rs`<br>`miku/crates/miku-cli/src/live_gateway/agent_tool_exec.rs`<br>`miku/crates/miku-cli/src/live_gateway/extended_routes.rs`<br>`miku/crates/miku-cli/src/live_gateway/api_handler.rs` | Any file in `miku/crates/miku-ui/` or `android/` |
| **Agent 4 (Android Link)** | `android/app/src/main/kotlin/dev/miku/link/**`<br>`android/build.gradle.kts`<br>`android/app/build.gradle.kts` | Any file in `miku/` or `src/` |
| **Agent 5 (Verification & Cockpit)** | `src/**`<br>`public/captures/**`<br>`tests/**` or Playwright test runner scripts | Any production source file in `miku/` or `android/` |
