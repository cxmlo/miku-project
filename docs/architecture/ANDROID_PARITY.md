# Matriz de Paridad Island <-> Android Miku-Link (Paso 4.9 · F-17)

- **Núcleo compartido Rust (`miku/crates/miku-mobile-core`)**:
  - Exportación C-ABI / UniFFI (`src/miku_mobile.udl` + `src/uniffi_ffi.rs`).
  - Verifica bit a bit `MIKU_REQ_HASH_V1|agent=...|cap=...|target=...|args=...` con SHA-256 (`compute_req_hash_v1`) contra los parámetros exactos mostrados en pantalla antes de autorizar `Allow`.
  - Bloquea `Allow` cuando `pc_only == true` tanto en cliente Android como en el servidor `GatekeeperEngine::resolve_permission_decision`.
  - Exige `BiometricProof` hardware (`AndroidX BiometricPrompt`) ligado al `req_hash` exacto para `Allow`; `Deny` opera en 1 toque sin biometría.
  - Prohíbe almacenar API keys de proveedores LLM (`gsk_`, `sk-ant-`, `sk-proj-`, etc.).

| Área | Island Escritorio (Windows) | Android Miku-Link (`android/`) | Regla de Paridad / Seguridad |
| :--- | :--- | :--- | :--- |
| **Permisos (`Allow`/`Deny`)** | Tarjeta directa en Island | Tarjeta Compose + `BiometricPrompt` para `Allow`; 1 toque `Deny` | `pc_only: true` rechazado en móvil y servidor; TTL expirado -> `Deny` |
| **Feed de Agentes** | Stream vivo del `EventBus` | `AgentFeedScreen` sobre WebSocket TLS pinned | Payloads idénticos `MikuLinkMessageV1::AgentEvent` |
| **Chat + Cola Offline** | Prompt directo en sesión | Cola local cuando hay corte, flush al reconectar | Replay ordenado vía `MikuLinkMessageV1::Command(action="prompt")` |
| **Voz (PTT)** | `Alt+Space` -> Groq Whisper en PC | Botón PTT remoto hacia la sesión del PC | El móvil jamás almacena claves Groq/OpenAI/Anthropic |
| **Kill Switch Global** | `Ctrl+Alt+Shift+K` | Barra roja fija `KILL SWITCH` | Revoca sesiones remotas y detiene agentes `< 100 ms` |
| **Compilación APK CI** | Crate `miku-mobile-core` MIT | `.github/workflows/android-apk.yml` + `android/` SDK 36 / NDK 27 | `cargo-ndk` (`arm64-v8a`, `x86_64`) + `gradle testDebugUnitTest assembleDebug` |
