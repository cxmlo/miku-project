# Guía de Usuario y Diseño: Instalador Seguro de Hooks (`miku hook install` / `miku hook restore`)

- **Micro-paso:** `3.13` (*Instalador seguro de hooks con copia de seguridad, diff visible y rollback — Lección Coucou / EX-07*)
- **Estado:** `VERIFICADO` (`2026-11-10` en Windows 11 + GitHub Actions CI run `42802320136` en commit `820e313c52d2cd72012186e8395ae08b13bdd9e9`)
- **Crate principal:** `miku/crates/miku-cli` (`src/hook_cmd/mod.rs` [587 líneas], `src/hook_cmd/merge_and_diff.rs` [299 líneas], `src/hook_cmd/tests.rs` [479 líneas])
- **Presupuesto de rendimiento:** Operación puntual `< 20 ms` (verificado en Windows 11 en 3 corridas focales consecutivas: `4.772 ms`, `2.509 ms` y `4.128 ms` incluyendo la confirmación dentro del cronómetro).

---

## 1. Propósito (La Lección Coucou)

Al integrar un orquestador de agentes con herramientas del desarrollador como **Claude Code** (`~/.claude/settings.json`) o **Cursor IDE** (`settings.json` / `mcp.json`), sobrescribir el archivo de configuración completo o usar plantillas destructivas puede borrar preferencias críticas del usuario, servidores MCP de terceros o hooks preexistentes.

`miku hook install` y `miku hook restore` garantizan por contrato:
1. **Validación previa sin tocar disco:** Si `settings.json` tiene un error de sintaxis JSON previo, la operación se detiene **antes** de crear copias o modificar archivos e informa la **línea y columna exactas** del error (redactando cualquier valor sensible entre comillas).
2. **Fusión quirúrgica e idempotente (`miku-orchestrator`):** Solo se inserta o actualiza la entrada propiedad de Miku (`"id": "miku-orchestrator"`). Todas las demás claves del usuario, hooks ajenos (`PreToolUse`, `PostToolUse`, `Notification`, `Stop`, etc.) y servidores MCP de terceros permanecen idénticos en orden y valor. Ejecutar `install` repetidas veces es **idempotente** (`idempotent_noop = true`) y jamás duplica hooks.
3. **Copia de seguridad `.bak` verificada antes de escribir:** Antes de pedir confirmación o modificar `settings.json`, Miku crea una copia fechada `<settings.json>.<timestamp_ms>.bak` y verifica byte por byte que su contenido coincide exactamente con el archivo original.
4. **Diff unificado visible y confirmación explícita obligatoria:** Muestra en pantalla el diff unificado (`--- a/settings.json`, `+++ b/settings.json`, `@@ ... @@`) y solicita confirmación interactiva explícita (`[y/N]`). No existe confirmación automática silenciosa.
5. **Cancelación sin escritura:** Si el usuario responde `N` (o cancela), el archivo `settings.json` activo **no se modifica en absoluto**.
6. **Restauración byte por byte (`miku hook restore`):** Restaura los bytes exactos del archivo `.bak` (preservando finales de línea CRLF/LF, espaciado personalizado y hash SHA-256 original) tras validar que el backup sea JSON íntegro y pertenezca a la jaula de rutas autorizada (`CanonicalPathJail`).

---

## 2. Uso desde la CLI

### 2.1 Instalar el hook de Miku (`miku hook install`)

Para **Claude Code**:
```powershell
cargo run --manifest-path miku/Cargo.toml -p miku-cli --bin miku-cli -- `
  hook install `
  --agent claude-code `
  --config-root "C:/Users/tu_usuario/.claude" `
  --settings "C:/Users/tu_usuario/.claude/settings.json"
```

Para **Cursor IDE**:
```powershell
cargo run --manifest-path miku/Cargo.toml -p miku-cli --bin miku-cli -- `
  hook install `
  --agent cursor `
  --config-root "C:/Users/tu_usuario/.cursor" `
  --settings "C:/Users/tu_usuario/.cursor/settings.json"
```

Opciones adicionales:
- `--backup <ruta/al/archivo.bak>`: Permite especificar una ruta explícita dentro de `--config-root` para la copia `.bak`. Si se omite, Miku genera automáticamente `<settings.json>.<epoch_ms>.bak`.

### 2.2 Flujo de Backup, Diff, Confirmación y Cancelación

Al ejecutar `miku hook install`:
1. **Confinamiento y Gatekeeper:** `CanonicalPathJail` verifica que `--settings` y `--backup` residan estrictamente dentro de `--config-root`, rechaza enlaces simbólicos (`SymlinkForbidden`) y consulta `GatekeeperEngine` (`Capability::FsRead` y `Capability::FsWrite`).
2. **Respaldo `.bak` verificado:** Se escribe la copia `.bak` y se comprueba byte por byte contra el `settings.json` original.
3. **Inspección del Diff Unificado:** Se imprime en la terminal el diff unificado completo:
   ```diff
   --- a/settings.json
   +++ b/settings.json
   @@ -1,4 +1,29 @@
    {
      "theme": "dark",
   +  "hooks": {
   +    "PreToolUse": [
   +      {
   +        "command": "miku-cli hook stdio --agent claude-code --event pre_tool_use",
   +        "id": "miku-orchestrator",
   +        "matcher": "*",
   +        "timeout_ms": 15,
   +        "type": "command"
   +      }
   +    ]
   +  }
    }
   Backup created & verified: C:/Users/tu_usuario/.claude/settings.json.1700000030000.bak
   Apply Miku hook changes to settings.json? [y/N]:
   ```
4. **Confirmación o Cancelación:**
   - Si escribes `y` o `yes`: Miku vuelve a verificar la integridad de la copia `.bak`, aplica el cambio en `settings.json` e imprime `MIKU_HOOK_INSTALLED|...`.
   - Si presionas `Enter` o escribes `n`: Miku aborta la escritura, deja tu `settings.json` activo intacto byte por byte e imprime `MIKU_HOOK_CANCELLED|...`.

### 2.3 Restaurar una copia de seguridad (`miku hook restore`)

Para restaurar el último backup `.bak` fechado automáticamente:
```powershell
cargo run --manifest-path miku/Cargo.toml -p miku-cli --bin miku-cli -- `
  hook restore `
  --config-root "C:/Users/tu_usuario/.claude" `
  --settings "C:/Users/tu_usuario/.claude/settings.json"
```

Para restaurar un archivo `.bak` específico:
```powershell
cargo run --manifest-path miku/Cargo.toml -p miku-cli --bin miku-cli -- `
  hook restore `
  --config-root "C:/Users/tu_usuario/.claude" `
  --settings "C:/Users/tu_usuario/.claude/settings.json" `
  --backup "C:/Users/tu_usuario/.claude/settings.json.1700000030000.bak"
```

Antes de restaurar, `miku hook restore` valida que el archivo `.bak` exista dentro del directorio autorizado y contenga un objeto JSON válido. Si el `.bak` está corrupto, informa la línea exacta del error y mantiene intacto el `settings.json` activo.

---

## 3. Manejo Seguro de Errores y Cero Fuga de Secretos

- **JSON inválido en `settings.json`:** Si el archivo tiene una coma colgante o un token inválido en la línea `N`, la operación falla antes de crear `.bak` o tocar `settings.json`:
  `invalid settings.json syntax at line N, column M: ...`
- **Redacción automática (`sanitize_json_error_reason`):** Cualquier literal entre comillas reportado por el parser JSON se reemplaza por `"[REDACTED]"` para que ninguna API key o token adyacente al error de sintaxis se filtre en la consola o en los logs.

---

## 4. Ledger de Pruebas TDD (`LEDGER-3.13-1..7`)

| ID Ledger | Prueba en `miku-cli` | Cobertura Verificada |
| :--- | :--- | :--- |
| `LEDGER-3.13-1` | `hook_cmd::tests::test_merge_miku_hook_preserves_all_existing_user_keys_and_values` | Fusiona la configuración de Miku en Claude Code y Cursor preservando intactas todas las claves y objetos previos del usuario. |
| `LEDGER-3.13-2` | `hook_cmd::tests::test_repeated_hook_install_is_idempotent_and_never_duplicates_hooks` | Ejecutar `install` múltiples veces es idempotente (`idempotent_noop == true`) y jamás duplica entradas de hooks. |
| `LEDGER-3.13-3` | `hook_cmd::tests::test_backup_created_and_validated_before_write_and_diff_shown_before_confirmation` | Comprueba con un prompter instrumentado que `.bak` existe y está validado en disco y que el diff unificado aparece antes de pedir confirmación y antes de escribir `settings.json` (`< 20 ms`). |
| `LEDGER-3.13-4` | `hook_cmd::tests::test_user_cancellation_leaves_active_settings_file_completely_unmodified` | Si el usuario cancela en la confirmación, `settings.json` no sufre ninguna escritura y conserva sus bytes originales. |
| `LEDGER-3.13-5` | `hook_cmd::tests::test_hook_restore_recovers_backup_byte_for_byte_and_completes_under_20ms` | `restore` recupera la copia de seguridad byte por byte (incluyendo CRLF, espaciado a medida y SHA-256 idéntico) en `< 20 ms`, tanto por API como por CLI. |
| `LEDGER-3.13-6` | `hook_cmd::tests::test_invalid_json_invalid_path_or_corrupt_backup_fails_safely_with_exact_line_and_zero_secret_leak` | JSON inválido, path traversal, denegación de Gatekeeper o backup corrupto fallan de forma cerrada sin tocar el archivo activo, reportando la línea exacta y con 0 fugas de secretos. |
| `LEDGER-3.13-7` | `hook_cmd::tests::test_only_miku_entry_is_modified_while_foreign_hooks_and_mcp_servers_remain_identical_and_sub_600_lines` | Hooks ajenos (`PreToolUse`, `PostToolUse`, `Stop`) y servidores MCP ajenos permanecen idénticos en posición y valor; audita que todos los `.rs` de `miku-cli/src` tengan `< 600` líneas. |

### Evidencia Real de Verificación (Windows 11 + GitHub Actions · `2026-11-10`)
- **`hook_cmd::tests::` (3 corridas consecutivas):** `7/7` aprobadas en cada corrida (`HOOK_FOCAL_3X_EXIT_CODE=0`). Tiempos totales medidos por `HOOK_INSTALL_TIMINGS_MS` (incluyendo la confirmación dentro del cronómetro): **`4.772 ms`**, **`2.509 ms`** y **`4.128 ms`** (`< 20.0 ms`).
- **`miku-cli` en paralelo:** `22` tests de biblioteca + `1` de `miku-cli` + `1` de `miku-license-audit` (`24/24`, `CLI_PARALLEL_EXIT_CODE=0`).
- **Workspace serial (`--workspace -- --test-threads=1 < NUL`):** `193 passed, 0 failed` (`WORKSPACE_EXIT_CODE=0`).
- **`miku-agent harness::` (fuera del sandbox por requerir loopback):** `12/12` aprobadas.
- **Benchmarks y auditoría modular de `miku-ui`:** benchmarks de voz (`< 25.0 ms`) y kill switch (`< 100.0 ms`) aprobados; auditoría `miku-ui` aprobada (`59` archivos `.rs`, máximo `538` líneas).
- **Clippy estricto (`--workspace --all-targets -- -D warnings`):** `CLIPPY_EXIT_CODE=0`, sin diagnósticos Clippy (Cargo emitió advertencias separadas sobre hard linking del caché incremental y usó fallback de copia).
- **GitHub Actions CI:** verde en el commit `820e313c52d2cd72012186e8395ae08b13bdd9e9` (`https://github.com/cxmlo/miku-project/actions/runs/42802320136`).
