# Plan de Implementación · Fase 1 · Micro-paso 1.7 (`miku-platform`: Autostart en `HKCU\...\Run` Sandboxed, Arranque Silencioso y Benchmark `< 1.0 s`)

## 1. Contexto y Auditoría de Cierre de 1.6 (`VERIFICADO` -> `14/57 = 24.6%` Global · `6/9 = 66.7%` Fase 1)

El **Micro-paso 1.6 (`miku-memory` · Context Engine & Working Memory)** ha quedado **VERIFICADO al 100%** en tu portátil (Windows 11, 16 GB RAM, RTX 3050):
- **`cargo test -p miku-memory`**: `5/5` tests verdes (`4` en `src/lib.rs` + `1` e2e en `src/main.rs`).
- **`cargo clippy --all-targets -- -D warnings`**: `0 warnings` en los 7 crates del workspace `miku/` (`2.76 s`).
- **`cargo run --release -p miku-memory` (20 repeticiones reales)**:
  - Tamaño del binario release: **`0.53 MB`** (`BUD-09 < 15.0 MB` -> **CUMPLE**).
  - Contadores de contexto: `chunks = 80`, `gk_blocked = 40`, `evicted_turns = 120` (**CUMPLE**).
  - Invariantes de ahorro de tokens (`F-09`): `prefix_hash_stable = 20/20`, `budget_respected = 20/20`, `audit_ok = 20/20` (prefijo inmutable byte a byte verificado).
  - Latencia de ciclo de contexto: **`p50 = 0.2869 ms | p95 = 0.4080 ms`** (umbral `<= 2.0 ms` -> **CUMPLE** con amplio margen).
  - Memoria privada del worker (`BUD-01`): **`p95 = 0.74 MB`** (umbral `<= 30.0 MB` -> **CUMPLE**).
  - Procesos huérfanos: **`0`** (verificados por tupla `(pid, name_lower, creation_date)`).
  - Veredicto: **`VERIFIER: OK`**.

Con `1.1` a `1.6` verificados (`14/57` micro-pasos globales = **`24.6%`**, `6/9` de Fase 1 = **`66.7%`**), abrimos el **Micro-paso 1.7**: **`miku-platform` (Registro de inicio automático con Windows en `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` con backend sandboxed, arranque silencioso `--background` / `--headless` y benchmark de inicio en frío `< 1.0 s`)**.

---

## 2. Ficha Técnica del Micro-paso 1.7

```text
ID: 1.7
Objetivo: Implementar en miku/crates/miku-platform el gestor de inicio automático con Windows (HKCU\Software\Microsoft\Windows\CurrentVersion\Run) desacoplado mediante un trait AutostartBackend (usando un backend simulado/sandboxed en todos los tests para no modificar jamás el registro real del sistema sin consentimiento explícito), el modo de arranque silencioso en segundo plano (--background / --headless con flag Win32 CREATE_NO_WINDOW) y el benchmark de inicio en frío (< 100.0 ms hasta socket WebSocket listo / < 1.0 s presupuesto total BUD-08).
Entregable usable: Crate miku/crates/miku-platform (biblioteca + binario de verificación) conectado a miku-protocol, miku-core y miku-gateway que valida y escapa rutas de ejecutables en Windows (con espacios y barras invertidas `\`), gestiona el ciclo enable/status/disable de autostart en un sandbox determinista, aplica los flags de ejecución silenciosa sin ventana de consola y audita 20 arranques reales en frío midiendo el tiempo exacto desde CreateProcess hasta el handshake WebSocket miku-link v1 completo en 127.0.0.1.
Tests a escribir primero (TDD):
  1. LEDGER-1.7-1 (Unitario): test_windows_run_command_quoting_escapes_spaces_backslashes_and_rejects_injection
  2. LEDGER-1.7-2 (Contrato): test_sandboxed_registry_backend_lifecycle_enable_status_disable_idempotency
  3. LEDGER-1.7-3 (Unitario/Seguridad): test_cli_launch_mode_parser_enforces_headless_background_flags_and_no_window_creation_flags
  4. LEDGER-1.7-4 (Integración): test_cold_start_headless_daemon_reaches_websocket_ready_state_and_shuts_down_cleanly
  5. LEDGER-1.7-5 (Benchmark E2E): e2e_miku_platform_cold_start_20_reps_sub100ms_ws_ready_sub1s_total_and_zero_orphans
Criterios de aceptación:
  - Cero efectos secundarios en el registro real de Windows durante tests y benchmarks: todas las operaciones de prueba usan SandboxedRegistryBackend en memoria verificando la ruta exacta HKCU\Software\Microsoft\Windows\CurrentVersion\Run y el nombre de valor "MikuDesktopOrchestrator".
  - Formateo estricto de rutas Windows: envuelve siempre la ruta absoluta en comillas dobles ("C:\Program Files\Miku\miku.exe" --background --headless), preserva barras invertidas canónicas de Windows y rechaza rutas relativas, rutas vacías, bytes nulos, saltos de línea, comillas embebidas o metacaracteres de inyección (`&`, `|`, `;`, `>`).
  - Arranque silencioso real: lanzamiento del proceso worker con CREATE_NO_WINDOW (0x08000000) en Windows y banderas `--background --headless` sin abrir ninguna ventana de consola adicional ni robar foco.
  - Benchmark de inicio en frío (20 repeticiones reales): mide el tiempo desde `Command::spawn()` hasta que el daemon hijo levanta `miku-core` + `miku-gateway` en `127.0.0.1` y completa un handshake autenticado `hello -> hello_ack` (`p95 < 100.0 ms` para BUD-07 y `< 1000.0 ms` para BUD-08).
Presupuesto de rendimiento afectado:
  - BUD-07: Arranque del daemon en frío hasta aceptar conexiones WebSocket < 100.0 ms
  - BUD-08: Arranque con Windows en frío < 1000.0 ms (< 1.0 s)
  - BUD-01: RAM privada del worker en segundo plano <= 30.0 MB
  - BUD-09: Tamaño del binario release miku-platform.exe < 15.0 MB
Riesgos y mitigación:
  - Riesgo de ensuciar HKCU\...\Run real de tu portátil al correr `cargo test` o `cargo run` -> el trait `AutostartBackend` inyecta `SandboxedRegistryBackend` por defecto; el backend Win32 real solo se invoca bajo comando explícito de usuario final con flag de confirmación.
  - Rutas en Windows con espacios (`C:\Users\Nombre Apellido\...`) que fallan al arrancar en `Run` si no llevan comillas exactas -> validador y constructor determinista `build_windows_run_command`.
Dependencias: 1.1, 1.2, 0.5
Definición de terminado: 5/5 tests verdes + cargo clippy --all-targets -- -D warnings con 0 warnings + salida real de las 20 repeticiones pegada por el usuario.
```

---

## 3. Arquitectura de `miku/crates/miku-platform`

### 3.1 Estructura en el Workspace (`miku/`)
```text
miku/
├── Cargo.toml                           # Workspace: añade "crates/miku-platform" (8 crates en total)
└── crates/
    ├── miku-protocol/                   # VERIFICADO en 1.1
    ├── miku-core/                       # VERIFICADO en 1.1
    ├── miku-gateway/                    # VERIFICADO en 1.2
    ├── miku-gatekeeper/                 # VERIFICADO en 1.3
    ├── miku-agent/                      # VERIFICADO en 1.4
    ├── miku-storage/                    # VERIFICADO en 1.5
    ├── miku-memory/                     # VERIFICADO en 1.6
    └── miku-platform/                   # NUEVO en 1.7
        ├── Cargo.toml
        └── src/
            ├── lib.rs                   # AutostartManager + SandboxedRegistryBackend + Windows Path Quoting + Headless Launch Config + 4 tests
            └── main.rs                  # Binario miku-platform (Coordinador + Worker --background --headless 20 reps) + test e2e
```

### 3.2 Componentes Clave en `miku/crates/miku-platform/src/lib.rs`

1. **Constantes de Contrato del Registro de Windows**:
   - `HKCU_RUN_SUBKEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run"`
   - `MIKU_RUN_VALUE_NAME: &str = "MikuDesktopOrchestrator"`
   - `WIN32_CREATE_NO_WINDOW: u32 = 0x0800_0000`

2. **Constructor y Validador Seguro del Comando `Run` (`build_windows_run_command` & `verify_windows_run_command`)**:
   - Normaliza separadores a barra invertida de Windows (`\`), exige ruta absoluta con letra de unidad (`C:\...`) y extensión `.exe`, prohíbe caracteres de control/inyección (`\0`, `\r`, `\n`, `"`, `&`, `|`, `;`, `>`, `<`, `` ` ``) y produce exactamente:
     ```text
     "C:\Path\With Spaces\miku.exe" --background --headless
     ```
   - Incluye el parser inverso `parse_windows_run_command` que verifica que un valor leído del registro corresponde a una ruta `.exe` válida entrecomillada seguida de los flags `--background --headless`.

3. **Trait `AutostartBackend` y `SandboxedRegistryBackend`**:
   - Define las operaciones `get_run_value(&self, subkey: &str, value_name: &str)`, `set_run_value(&mut self, subkey: &str, value_name: &str, command: &str)` y `delete_run_value(&mut self, subkey: &str, value_name: &str)`.
   - `SandboxedRegistryBackend`: almacén determinista en memoria que simula exactamente la jerarquía `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`, registrando contadores de auditoría (`writes_count`, `deletes_count`, `reads_count`) y garantizando **cero escrituras en el registro real de tu máquina** durante `cargo test` y `cargo run`.
   - `AutostartManager<B: AutostartBackend>`: expone `enable(exe_path)`, `disable()`, `status(expected_exe_path) -> AutostartState` (`Disabled`, `EnabledMatching`, `EnabledDrift { current_command }`).

4. **Parser de Modo de Arranque Silencioso (`LaunchConfig` & `configure_headless_command`)**:
   - Parsea argumentos CLI (`--background`, `--headless`, `--worker`, `--port`, `--hold-for-ram`).
   - En Windows (`#[cfg(target_os = "windows")]`), aplica `std::os::windows::process::CommandExt::creation_flags(WIN32_CREATE_NO_WINDOW)` al `std::process::Command` para garantizar que el proceso hijo en segundo plano arranque con **0 ventanas de consola** y sin parpadeo visual.

---

## 4. Los 5 Tests del Test Ledger (TDD Primero: Rojo -> Verde)

1. **`LEDGER-1.7-1` (`unitario`) — `tests::test_windows_run_command_quoting_escapes_spaces_backslashes_and_rejects_injection`**:
   - Verifica que rutas con espacios y barras mixtas (`C:/Program Files/Miku Desktop/miku.exe`) se normalizan a `"C:\Program Files\Miku Desktop\miku.exe" --background --headless`, y que rutas relativas, sin `.exe` o con intentos de inyección (`miku.exe" & calc.exe`) son rechazadas con `PlatformError::InvalidExecutablePath`.
2. **`LEDGER-1.7-2` (`contrato`) — `tests::test_sandboxed_registry_backend_lifecycle_enable_status_disable_idempotency`**:
   - Verifica en `SandboxedRegistryBackend` el flujo `Disabled -> enable() -> EnabledMatching -> detección de EnabledDrift si cambia la ruta del binario -> disable() -> Disabled`, comprobando idempotencia al deshabilitar dos veces y que la llave objetivo es exactamente `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`.
3. **`LEDGER-1.7-3` (`unitario`) — `tests::test_cli_launch_mode_parser_enforces_headless_background_flags_and_no_window_creation_flags`**:
   - Verifica el parser `LaunchConfig::from_args` y comprueba que un lanzamiento con `--background --headless` activa `is_silent_background() == true` y asigna `creation_flags == 0x0800_0000` (`CREATE_NO_WINDOW`).
4. **`LEDGER-1.7-4` (`integración`) — `tests::test_cold_start_headless_daemon_reaches_websocket_ready_state_and_shuts_down_cleanly`**:
   - Arranca el runtime `tokio::current_thread` en modo headless, inicializa `miku-core` + `miku-gateway` en `127.0.0.1:0`, conecta un cliente WebSocket `miku-link v1`, verifica `hello -> hello_ack` + `ping -> pong` y ejecuta apagado limpio con `0` conexiones residuales.
5. **`LEDGER-1.7-5` (`benchmark`) — `tests::e2e_miku_platform_cold_start_20_reps_sub100ms_ws_ready_sub1s_total_and_zero_orphans`**:
   - Lanza 20 procesos reales en frío con `CREATE_NO_WINDOW` (`--worker --background --headless`), midiendo desde el instante anterior a `cmd.spawn()` hasta recibir `READY_PORT` y completar el handshake WebSocket `miku-link v1` desde el coordinador (`cold_start_ws_ready_ms` con umbral `p95 < 100.0 ms` y `< 1000.0 ms`), además de RAM privada `<= 30.0 MB` (`--hold-for-ram`), binario `< 15.0 MB` y `0` procesos huérfanos.

---

## 5. Directivas Preventivas de Higiene para Windows

- **Cabecera obligatoria en `lib.rs` y `main.rs`**:
  ```rust
  #![allow(
      unknown_lints,
      clippy::too_many_arguments,
      clippy::manual_range_contains,
      clippy::needless_range_loop,
      clippy::large_enum_variant,
      clippy::chunks_exact_to_as_chunks,
      clippy::type_complexity,
      clippy::invisible_characters
  )]
  ```
- **Compatibilidad de rutas Windows**: uso de raw strings `r"..."` y pruebas explícitas con `\` y espacios en nombres de carpetas (`C:\Users\Camila Miranda\AppData\Local\Programs\Miku\miku.exe`).
- **Regla de Integridad**: todas las métricas numéricas de `1.7` permanecen en `PENDIENTE-LOCAL` hasta tu ejecución real en consola.
