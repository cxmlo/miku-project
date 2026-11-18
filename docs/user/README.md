# Guía de Usuario de Miku

Bienvenido a la documentación operativa de **Miku Desktop Orchestrator**. Esta guía cubre la configuración del proyecto, la creación de Skills seguras, la integración con IDEs/MCP y el acceso remoto cifrado desde Android.

---

## 1. Instalación y ciclo de vida en Windows
- Instala `miku.exe` en `%LOCALAPPDATA%\Programs\Miku\miku.exe` con `miku.exe install` (sin permisos de administrador).
- Actualiza únicamente tras confirmar versión + digest SHA-256 mediante `miku.exe update --manifest release.json`.
- Desinstala con `miku.exe uninstall` (ofrece conservar o purgar `%LOCALAPPDATA%\Miku`).
- Especificación completa: [Instalador ligero y actualizador SHA-256 (F-24)](../design/F-24.md).

---

## 2. Guía de creación de Skills (`skill.toml`)

Las Skills de Miku son plugins locales enjaulados (`CanonicalPathJail`), versionados con SemVer estricto y validados en frío sin ejecutar código durante la carga. Cada Skill declara el límite superior de capacidades (`capabilities`) que puede pedir; `GatekeeperEngine` sigue aplicando `Deny > Ask > Allow` sobre cada invocación.

### Estructura de carpeta recomendada
```text
skills/mi-skill-auditor/
├── skill.toml
└── scripts/
    └── run.ps1
```

### Manifiesto `skill.toml` válido
```toml skill-toml
schema_version = 1

[skill]
id = "git-diff-auditor"
name = "Git Diff Auditor"
version = "1.0.0"
description = "Inspecciona diferencias locales de trabajo dentro de la jaula sin acceso de red"
entrypoint = "scripts/run.ps1"
capabilities = ["fs_read"]
allowed_targets = ["src/**", "crates/**", "docs/**"]

[limits]
timeout_ms = 5000
max_output_bytes = 65536
isolated_process = true
```

### Reglas de seguridad de una Skill
1. **`schema_version = 1`** obligatorio y rechazo de campos desconocidos (`deny_unknown_fields`).
2. **`entrypoint`** relativo al directorio de la Skill; se prohíben rutas absolutas, `..` o enlaces simbólicos (`SymlinkForbidden`).
3. **`isolated_process = true`** obligatorio para aislamiento de ejecución y cuotas de salida (`<= 1 MiB`).
4. **Hot-reload transaccional:** si editas `skill.toml` y introduces un error de sintaxis o una capacidad prohibida, el último estado válido permanece activo (`Shadow Buffer`).

---

## 3. Conectar servidores MCP externos (`[mcp_servers.*]`)

Puedes declarar herramientas MCP externas dentro de tu `.miku.toml`. Cada llamada pasa por el puente de `miku-mcp` bajo el Gatekeeper y registro de auditoría SHA-256:

```toml mcp-servers
[mcp_servers.filesystem_readonly]
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "./src"]
timeout_ms = 5000
idle_ttl_ms = 30000
auto_start = false
```

---

## 4. Conectar tu IDE (Claude Code / Cursor)
Miku incluye un instalador seguro con respaldo `.bak`, diff unificado visible y rollback byte a byte:
- Consulta la guía paso a paso en [Instalador Seguro de Hooks](hooks-installer.md).

---

## 5. Acceso remoto fuera de casa (Tailscale Mesh + Push-Wake opcional)
- **Canal principal (100 % gratuito y cifrado WireGuard, ADR-013):** sigue [Acceso Remoto con Tailscale Mesh](tailscale-tunnel.md). Si prefieres un túnel HTTP/S propio o proxy reverso equivalente en vez de Tailscale, Miku sigue exigiendo `wss://` con TLS + token efímero en `MikuLinkMessageV1::Hello` y Kill Switch local.
- **Notificaciones Push sin PII (opcional):** consulta [Despertador Push sin PII](push-wake-relay.md).
- **Personalización visual del Avatar y accesorios:** consulta [Catálogo de Accesorios UI](../ui/CATALOGO_ACCESORIOS.md).
