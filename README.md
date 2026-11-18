# Miku Desktop Orchestrator

Orquestador local de agentes autónomos con interfaz de escritorio ligera tipo Dynamic Island en Windows 11, protocolo `miku-link v1`, seguridad fail-closed (`miku-gatekeeper`) y acompañante Cockpit web.

- **Licencia:** 100 % MIT ([LICENSE](LICENSE))
- **Binario único:** `miku.exe` (< 15 MB en release con `lto = true`; ~1.95 MB verificado)
- **Presupuestos duros:** RAM privada en reposo `<= 30 MB`, arranque frío WebSocket `<= 100 ms`, CPU ocioso `0.0 %`, 0 procesos huérfanos WebView2 (`msedgewebview2.exe`)

---

## Cómo trabajar en local (después de clonar)

Si clonaste este repositorio en una máquina nueva, sigue estos pasos para dejar el entorno listo:

### 1. Requisitos previos
- **Git**
- **Rust (stable)** con `clippy` y `rustfmt` (`rustup toolchain install stable --component clippy,rustfmt`) — necesario para compilar `miku.exe` y los crates en `miku/`.
- **Windows 11** con **Microsoft Edge WebView2 Evergreen** (incluido por defecto en Windows 11) para la interfaz Dynamic Island de `miku.exe`.
- **Node.js (v20+ / v22+) y npm** — necesarios para el Cockpit web en la raíz (`src/`).
- *(Opcional, solo si vas a compilar la app Android `android/`)*: **JDK 17**, **Android SDK / NDK r27c**, targets de Rust (`rustup target add aarch64-linux-android x86_64-linux-android`) y `cargo-ndk` (`cargo install cargo-ndk --locked`).

### 2. Clonar y configurar variables de entorno
```powershell
git clone https://github.com/cxmlo/miku-project.git
cd miku-project
```
Si vas a usar funciones del Cockpit web que requieren clave de Gemini, copia `.env.example` a `.env` y rellena tus valores:
```powershell
Copy-Item .env.example .env
```

### 3. Levantar y probar cada componente en local

- **Núcleo Rust y CLI (`miku.exe`):**
  ```powershell
  cargo test --manifest-path miku/Cargo.toml --workspace
  cargo clippy --manifest-path miku/Cargo.toml --all-targets -- -D warnings
  cargo build --manifest-path miku/Cargo.toml --release --bin miku
  ```
- **Cockpit web (`src/`):**
  ```powershell
  npm install
  npm run dev      # servidor local en http://localhost:3000
  npm run lint     # verificación de tipos TypeScript
  npm run build    # build de producción en dist/
  ```
- **App Android (`android/`, opcional):**
  ```powershell
  cargo ndk -t arm64-v8a -t x86_64 -o android/app/src/main/jniLibs build --manifest-path miku/Cargo.toml -p miku-mobile-core --release
  cd android
  .\gradlew.bat testDebugUnitTest assembleDebug
  ```

---

## Inicio rápido

### 1. Compilar y ejecutar diagnóstico local (Windows 11)
```powershell
cargo build --manifest-path miku/Cargo.toml --release --bin miku
.\miku\target\release\miku.exe doctor --json
.\miku\target\release\miku.exe ci-bench --runs 5 --json
```

### 2. Instalar en espacio de usuario (sin administrador)
```powershell
.\miku\target\release\miku.exe install
```
Instala `miku.exe` en `%LOCALAPPDATA%\Programs\Miku\miku.exe`, valida integridad por SHA-256 y crea `%LOCALAPPDATA%\Miku\`. Consulta el flujo completo de instalación, desinstalación y actualización verificada en [docs/design/F-24.md](docs/design/F-24.md).

### 3. Comandos principales de `miku.exe`
| Comando | Propósito |
|---|---|
| `miku.exe run [--json]` | Smoke completo del binario único unificado (13 crates) |
| `miku.exe doctor [--json]` | Diagnóstico de puerto loopback `47400`, WebView2 Evergreen, huérfanos, `.miku.toml` y presupuestos |
| `miku.exe demo [--json]` | Demostración visual Win32 de la Dynamic Island (Expanded / Idle / Alert / Avatar) |
| `miku.exe ci-bench --runs 5 --json` | Compuerta de rendimiento y presupuestos de CI |
| `miku.exe hook install\|restore ...` | Instalación quirúrgica y reversible de hooks en Claude Code / Cursor ([guía](docs/user/hooks-installer.md)) |
| `miku.exe secret set\|list\|delete ...` | Bóveda local DPAPI / `WindowsCredentialBackend` con memoria zeroizable |
| `miku.exe install\|uninstall\|update` | Ciclo de vida en Windows con consentimiento explícito y SHA-256 |

### 4. Cockpit web local (opcional para tablero de seguimiento)
```powershell
npm install
npm run lint
npm run build
```

---

## Configuración rápida (`.miku.toml`)

```toml miku-toml
[project]
name = "mi-repositorio"
root = "C:/Users/cxmluwu/antigravity/Prompt-Genie-2026-11-05-5baa8"
default_ttl_ms = 30000

[browser]
driver = "playwright"
profile_isolation = true

[allow]
fs_read = ["src/**", "docs/**", "README.md"]
shell = ["cargo test*", "npm run build*"]

[ask]
fs_write = ["src/**", "docs/**"]
shell = ["git push*"]
browser_domains = ["localhost", "127.0.0.1"]

[deny]
fs_read = [".env*", "secrets/**"]
shell = ["rm -rf*", "format *", "curl * | sh*"]
```

---

## Mapa de documentación
- **Guía de usuario, Skills y túnel remoto:** [docs/user/README.md](docs/user/README.md)
- **Guía de agentes custom (`agent.toml` / `SOUL.md`) y adaptadores:** [docs/agents/README.md](docs/agents/README.md)
- **Especificación del protocolo `miku-link v1`:** [docs/protocol/README.md](docs/protocol/README.md)
- **Paridad Android:** [docs/architecture/ANDROID_PARITY.md](docs/architecture/ANDROID_PARITY.md) y [docs/protocol/ANDROID_LINK.md](docs/protocol/ANDROID_LINK.md)
- **Instalador y actualizador SHA-256:** [docs/design/F-24.md](docs/design/F-24.md)
- **Seguridad y matriz de amenazas:** [SECURITY.md](SECURITY.md)
- **Cómo contribuir:** [CONTRIBUTING.md](CONTRIBUTING.md)
