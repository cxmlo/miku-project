# Checklist Manual Final · Miku Release `1.0.0` (Paso `5.6`)

> **Estado actual:** Automatización de release lista (`CHANGELOG.md` creado en raíz, `ci-bench` y `doctor 5/5` PASS sobre `miku/dist/miku.exe` SHA-256 `02bbbdb4647dddcb26330ef479d01fe42b794798bebe0e21703dc03e066f8317`) — **Fase 7 (`5/5`) y Fase 8 (`6/6`) VERIFICADAS**.
> **No crear etiquetas `miku-v1.0.0` ni `android-v1.0.0` hasta completar y tildar los puntos físicos §2–§5 en tu portátil y en tu móvil Android.**

---

## 1. Suite automatizada previa (ejecutar antes de probar en vivo)

```powershell
# 1. Cero warnings Clippy + auditoría MIT
cargo clippy --manifest-path miku/Cargo.toml --workspace --all-targets -- -D warnings
cargo run --release --manifest-path miku/Cargo.toml -p miku-cli --bin miku-license-audit

# 2. Suite completa del workspace (incluye fuzzing 100k, seguridad §8, instalador SHA-256 y docs)
cargo test --manifest-path miku/Cargo.toml --workspace -- --test-threads=1

# 3. Binario único release LTO (< 15 MB) + compuerta CI y doctor
cargo build --manifest-path miku/Cargo.toml --release --bin miku
.\miku\target\release\miku.exe ci-bench --runs 5 --json
.\miku\target\release\miku.exe doctor --json
```

---

## 2. Verificación física en Portátil Windows 11 (NVIDIA RTX 3050 · Multi-DPI)

- [ ] **Hover real y pastilla nativa (`miku.exe demo` / Modo C):**
  - Ejecutar `.\miku\target\release\miku.exe demo --visual-ms 12000` (o modo interactivo de la Island).
  - Confirmar visualmente que la etiqueta de texto del pill (`[DEMO] Idle Compact (248x44)`) **no se superpone** sobre el medidor de barras ni se corta horizontalmente.
  - Verificar expansión suavizada al pasar el cursor (`hover`), foco sin robar teclas a la ventana activa (`WS_EX_NOACTIVATE`) y transición fluida hacia `https://miku.local`.
- [ ] **Escala Multi-Monitor & DPI (`100 %`, `125 %`, `150 %`):**
  - Probar la Island expandida (`640x380` lógicos) cambiando la escala de pantalla de Windows (`Configuración -> Sistema -> Pantalla -> Escala`) entre `100 %`, `125 %` y `150 %` (y al arrastrar entre monitor integrado y monitor externo).
  - Confirmar 0 scroll horizontal, bordes limpios y nitidez de fuentes y SVG del avatar.
- [ ] **Modo Gaming Fullscreen (RTX 3050):**
  - Abrir un juego o aplicación DirectX/Vulkan en pantalla completa exclusiva (`RTX 3050`) mientras Miku corre en segundo plano.
  - Confirmar suspensión automática silenciosa en estado `Gaming` (`CPU = 0.0 %`), cero parpadeos/overlays sobre el juego y reanudación limpia al salir de pantalla completa.
- [ ] **Kill Switch físico (`Ctrl+Alt+Shift+K`):**
  - Presionar `Ctrl+Alt+Shift+K` durante actividad simulada/demo y confirmar detención inmediata de voz, audio ducking, navegador y sesiones en `< 100 ms` con `0` procesos `msedgewebview2.exe` residuales.

---

## 3. Verificación física en Móvil Android (`Miku-Link`)

- [ ] **Emparejamiento LAN por QR (`https://miku.local` -> Android):**
  - Escanear el QR SVG generado en la pestaña **Miku-Link** de la Island o Cockpit y confirmar handshake `hello -> hello_ack` (`miku-link v1`).
- [ ] **Acceso fuera de casa vía Tailscale Mesh (opcional Push-Wake):**
  - Conectar Windows y Android a tu Tailnet ([guía Tailscale](user/tailscale-tunnel.md)), probar desconectando Wi-Fi (usando datos móviles 4G/5G) y verificar reconexión autenticada `wss://<nodo>.ts.net:47400/ws`.
  - Si activas el despertador push ([Push-Wake](user/push-wake-relay.md)), comprobar entrega de los 3 campos sin PII (`agent_name`, `state`, `event_ts`).
- [ ] **Compuerta biométrica remota (F-18 / Sección 8):**
  - Enviar una solicitud de permiso simulada (`High` / `Critical`), verificar bloqueo automático hasta aprobar con huella/biometría en Android, y confirmar bloqueo absoluto `pc_only` en acciones restringidas al escritorio.

---

## 4. Instalador y Verificación SHA-256 (`F-24`)

- [ ] Probar flujo completo en espacio de usuario ([F-24](design/F-24.md)):
  ```powershell
  .\miku\target\release\miku.exe install --dry-run --json
  Get-FileHash .\miku\target\release\miku.exe -Algorithm SHA256
  ```
- [ ] Verificar tamaño final de `miku.exe` `< 15 MB` (~`1.95 MB` observado) y conservación/limpieza opcional de `%LOCALAPPDATA%\Miku` al probar `miku.exe uninstall`.

---

## 5. Cierre de Release `1.0.0` (solo tras completar §1–§4)

- [ ] Confirmar CI verde en GitHub Actions (`.github/workflows/ci.yml` incluyendo `cargo audit` y `cargo deny` en `windows-latest`).
- [ ] Redactar sección `[1.0.0]` en `CHANGELOG.md` con hashes `SHA256SUMS.txt` de `miku.exe` y APK Android.
- [ ] Actualizar paso `5.6` a `VERIFICADO` (`63 / 63 = 100.0%`) en `src/state/projectState.ts`.
- [ ] Crear y publicar los tags finales:
  ```powershell
  git tag -a miku-v1.0.0 -m "Miku Desktop Orchestrator 1.0.0"
  git tag -a android-v1.0.0 -m "Miku-Link Android Companion 1.0.0"
  git push origin miku-v1.0.0 android-v1.0.0
  ```
