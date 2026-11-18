# Changelog

Todas las versiones notables de **Miku Desktop Orchestrator** se documentan en este archivo bajo SemVer desde `0.1.0`.

## [1.0.0-rc1] — 2026-11-13

### Añadido y corregido
- **Eliminación total de `/cockpit` web**: retirado del Gateway HTTP (`404` garantizado), puente UI (`bridge.js`), menú de tray (`win32_tray.rs`), vista Island y ventana `Configuración · Miku` (`1080×780`).
- **Lanzador real de webs y apps Windows (`Capability::Browser`)**: nuevo módulo modular `live_gateway/app_catalog.rs` + `app_launcher.rs` para ejecutar comandos directos (`abrir pinterest`, `abre discord`, etc.) con `0` tokens LLM bajo Gatekeeper (`sandbox` permitido, `restricted` y Kill Switch bloqueados) y herramienta `open_app_or_url` para flujos multi-paso.
- **Cierre automático por clic fuera e inactividad OS**: `miku-ui/src/webview/cursor_tracker.rs` emite `outside_click` (`GetAsyncKeyState`) y `user_idle` (`GetLastInputInfo >= 15s`); `island/app.js` repliega la isla al hacer clic fuera, perder foco (`blur`) o vencer `autoclose`.
- **Limpieza visual de Isla, Dropdown y Chat**: barra superior depurada (`Overview` + `Chat`), expansión dinámica del HWND para evitar recorte en `#ag-chat-dd` (los 6 agentes + acciones caben sin clipping en `280px`) y `#crt-panel`, eliminación de mensajes sintéticos `"Router Local"` y `"Razonamiento 0.2s"` falsos, e inputs oscuros coherentes (`.set-input`) en Miku Phone Link.
- **Plan Cockpit (`src/` -> `dist/index.html`)**: panel interactivo **Plan Semana 1 por Días (Días 1–5)** y **Galería de Capturas Reales** (`public/captures/*.png`).

### Métricas verificadas del binario Release (`miku/dist/miku.exe`)
- Tamaño release: `3,400,192 bytes` (`3.24 MB` < `15.0 MB`)
- SHA-256 (`miku.exe`): `02bbbdb4647dddcb26330ef479d01fe42b794798bebe0e21703dc03e066f8317`
- `miku.exe ci-bench --runs 5 --json`: mediana arranque frío `31.21 ms`, mediana pill `8.28 ms`, mediana RAM `2.40 MB`, `0` procesos `msedgewebview2.exe` huérfanos.
- `miku.exe doctor --json`: `5/5 PASS` (`1.96 ms`).
- `miku.exe install --dry-run --json`: contrato verificado (`AppData\Local\Programs\Miku\miku.exe`).

### Pendiente antes de etiquetar `miku-v1.0.0` y `android-v1.0.0`
- Validación física en hardware del usuario según [`docs/RELEASE_CHECKLIST_1.0.0.md`](docs/RELEASE_CHECKLIST_1.0.0.md) §§2–5 (juego fullscreen RTX 3050, escalado multi-monitor 100%/125%/150% DPI y prueba QR LAN/Tailscale en teléfono Android).

## [0.1.0] — 2026-11-13
- Base modular en 15 crates Rust (`miku-protocol`, `miku-core`, `miku-gateway`, `miku-gatekeeper`, `miku-storage`, `miku-memory`, `miku-agent`, `miku-platform`, `miku-mcp`, `miku-skills`, `miku-browser`, `miku-tunnel`, `miku-ui`, `miku-mobile-core`, `miku-cli`), licencia 100% MIT y arquitectura Dual-Window (`Island` + `Configuración · Miku 1080×780`).
