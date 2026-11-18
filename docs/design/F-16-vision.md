# Diseño Técnico · Paso 4.3 (`F-16`): Visión de Pantalla Puntual Bajo Demanda con Gatekeeper

- **Estado:** Implementación TDD (`miku/crates/miku-ui/src/vision/`)
- **Objetivo:** Permitir que el usuario o un agente autorizado capture **exclusivamente bajo demanda** la ventana activa (`ActiveWindow`) o una región rectangular seleccionada (`SelectedRegion`), comprimiendo y redimensionando la imagen en memoria bajo límites estrictos de bytes sin grabar vídeo continuo ni escribir temporales en disco.

## 1. Principios de Seguridad y Privacidad
1. **Deny-by-Default & `Capability::Vision`:** Todo intento de captura pasa por `GatekeeperEngine` (`Capability::Vision`). Si `.miku.toml` fija `"deny"`, `"ask"` o no existe regla explícita, `ScreenVisionController` jamás abre captura sin aprobación explícita.
2. **Anti-TOCTOU (`req_hash`) y `pc_only`:** Cuando el perfil responde `AskPending`, el ticket contiene el objetivo exacto (`screen_capture:active_window` o `screen_capture:region:{x},{y},{w},{h}`) y genera un `VisionConfirmationPreview` con miniatura acotada para mostrar al usuario. Si un atacante altera el target antes de confirmar (`req_hash` distinto) o intenta aprobar desde sesión móvil una regla `pc_only`, `GatekeeperEngine` rechaza la operación.
3. **Kill Switch Global Inmediato:** Si el Kill Switch está disparado, `ScreenVisionController` rechaza toda captura y pone a cero cualquier buffer en curso (`zeroize_slice`).
4. **Cero Vídeo Continuo & Cero Temporales en Disco:**
   - `active_background_capture_streams() == 0` y `continuous_recording_forbidden == true`.
   - Cero escritura de capturas o miniaturas en disco (`zero_disk_temp_files == true`).
5. **Saneado XSS en UI (`DomSafeTextRender`):** El título de la ventana capturada y el resumen del objetivo se sanean mediante `sanitize_for_text_content` sobre `WebView2IpcBridge` (`textContent`).

## 2. Compresión en Memoria y Limpieza Post-Turno
- Redimensionado progresivo por promedio de área (`downscale_rgba_box`) y codificación PNG RGBA determinista sin dependencias externas (`encode_rgba_png` con CRC32 + zlib/DEFLATE sin compresión RFC 1951).
- Garantía física de límite de tamaño (`max_output_bytes`) y dimensiones máximas (`max_dimension_px`).
- El payload de turno (`TurnVisionPayload`) limpia automáticamente su buffer RGBA/PNG mediante `Drop` y `clear_after_turn()` (usando `write_volatile` + `compiler_fence`).

## 3. Matriz de Pruebas TDD (`LEDGER-4.3-1..6`)
- `LEDGER-4.3-1`: Bloqueo estricto cuando `vision` es `deny`, `ask` sin aprobar, ausencia de regla, Kill Switch activo, desajuste `req_hash` (TOCTOU) o aprobación `pc_only` desde móvil.
- `LEDGER-4.3-2`: Redimensionado y compresión PNG en memoria desde un frame 4K (`3840x2160`) respetando `max_dimension_px` y `max_output_bytes`, `0` archivos en disco y limpieza completa (`retained_bytes == 0`) al cerrar el turno y en `Drop`.
- `LEDGER-4.3-3`: Flujo `ask` -> `VisionConfirmationPreview` con miniatura acotada + saneado XSS de títulos hostiles (`<script>`, `<img onerror>`) enviado por `WebView2IpcBridge` -> aprobación explícita -> captura exitosa.
- `LEDGER-4.3-4`: Invariante cero vídeo continuo (`active_background_capture_streams() == 0`, prohibición de bucle continuo, fallo tipado si un backend reporta stream activo).
- `LEDGER-4.3-5`: Sonda Win32 real (`Win32GdiScreenVisionBackend`) capturando una región acotada o ventana activa sin fugas GDI (`DeleteDC`/`DeleteObject`/`ReleaseDC` RAII) produciendo un PNG válido.
- `LEDGER-4.3-6`: Auditoría modular `< 600` líneas en `miku-ui/src/vision/` y `miku-ui/src/`.
