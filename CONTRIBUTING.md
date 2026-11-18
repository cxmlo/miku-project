# Contribuir a Miku Desktop Orchestrator

¡Gracias por contribuir! Todo el código de Miku se publica bajo licencia **100 % MIT** ([LICENSE](LICENSE)).

---

## 1. Requisitos previos
- **Windows 10/11 (x64)** con **WebView2 Evergreen Runtime** instalado.
- **Rust toolchain estable** (`rustc`, `cargo`, `clippy`, `rustfmt`).
- **Node.js 22+ y npm** para el Cockpit web y los scripts de `tools/miku-push-wake/`.

---

## 2. Reglas de arquitectura y calidad obligatorias

1. **Disciplina TDD estricta:**
   - Ningún cambio de protocolo, seguridad o runtime se da por terminado sin su test automatizado previo en verde.
2. **Modularidad por límite de archivo:**
   - Todo módulo fuente en `miku/crates/miku-ui/src/` debe mantenerse **por debajo de 600 líneas**.
   - Se prohíben re-exportaciones comodín `pub use *;` en la fachada raíz de `miku-ui`.
3. **Presupuestos no negociables (verificados por CI):**
   - RAM privada de proceso en reposo: **`<= 30.0 MB`**.
   - Tamaño binario release (`miku.exe` con `lto = true`): **`< 15.0 MB`**.
   - Arranque de pastilla Win32 (`pill_ms`): **`<= 100.0 ms`**; arranque frío WebSocket: **`<= 100.0 ms`**.
   - Procesos WebView2 huérfanos tras cierre: **`0`**.
4. **Cero dependencias copyleft:**
   - Toda nueva dependencia Cargo debe aprobar `miku-license-audit` y `deny.toml` (MIT / Apache-2.0 / BSD / ISC / Zlib / MPL-2.0 compatibles sin copyleft fuerte).
5. **Seguridad fail-closed:**
   - Toda ruta pasa por `CanonicalPathJail`, todo origen WebView2 se restringe a `https://miku.local/*` y toda entrada CLI/WS no confiable rechaza campos desconocidos (`deny_unknown_fields`). Consulte [SECURITY.md](SECURITY.md).

---

## 3. Lista de verificación local antes de abrir un Pull Request

```powershell
# 1. Cero warnings en todo el workspace Rust
cargo clippy --manifest-path miku/Cargo.toml --workspace --all-targets -- -D warnings

# 2. Auditoría de licencias 100 % MIT
cargo run --release --manifest-path miku/Cargo.toml -p miku-cli --bin miku-license-audit

# 3. Suite completa del workspace (incluye fuzzers, auditoría sección 8 y validación de docs)
cargo test --manifest-path miku/Cargo.toml --workspace -- --test-threads=1

# 4. Compilación release y compuerta de rendimiento
cargo build --manifest-path miku/Cargo.toml --release --bin miku
.\miku\target\release\miku.exe ci-bench --runs 5 --json
.\miku\target\release\miku.exe doctor --json

# 5. Validación del Cockpit TypeScript + Push-Wake sin PII
npm run lint
npx tsx --test src/utils/cockpitLogic.test.ts
node --test tools/miku-push-wake/index.test.mjs
```

---

## 4. Documentación viva
Si agregas o modificas ejemplos `.miku.toml`, `skill.toml`, `agent.toml` o tramas `miku-link` en [README.md](README.md) o en `docs/`, etiqueta el bloque de código con su tipo correspondiente y ejecuta:
```powershell
cargo test --manifest-path miku/Cargo.toml -p miku-cli --test docs_validation
```
El test verifica automáticamente todos los enlaces internos entre archivos Markdown y ejecuta los parsers reales de Miku sobre cada bloque de ejemplo.
