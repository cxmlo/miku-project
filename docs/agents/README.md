# Guía de Agentes Personalizados e Integraciones (`agent.toml` & `SOUL.md`)

Miku permite definir subagentes declarativos aislados por proveedor, modelo, presupuesto de contexto y conjunto mínimo de herramientas autorizadas.

---

## 1. Arquitectura de un agente custom (`agent.toml`)

### Estructura mínima de carpeta
```text
agents/code-reviewer/
├── agent.toml
└── SOUL.md
```

### Manifiesto `agent.toml` válido
```toml agent-toml
schema_version = 1

[agent]
id = "code-reviewer"
display_name = "Miku Code Reviewer"
description = "Revisa cambios locales de Rust y TypeScript aplicando principio de menor privilegio"
provider = "deep_seek"
model = "deepseek-chat"
soul_path = "SOUL.md"
allowed_tools = ["fs_read", "memory_query"]
credential_aliases = []
max_steps = 8
max_context_tokens = 4096
```

---

## 2. Reglas clave de diseño y seguridad
1. **Carga perezosa (*Lazy Loading*):** Miku registra únicamente metadatos del descriptor al arranque; el archivo `SOUL.md` y su prefijo de sistema (`StaticSystemPrefix`) no consumen RAM hasta invocar al agente.
2. **Presupuesto de `SOUL.md` (`<= 1,024` tokens):** mantener las instrucciones de rol concisas evita desplazar la memoria FTS5 (RAG) ni el historial de conversación dentro de `max_context_tokens`.
3. **Principio de menor privilegio:** `allowed_tools` se valida contra `ToolRegistry` (`fs_read`, `fs_write`, `shell_exec`, `browser_action`, `memory_query`). Cualquier herramienta no declarada falla de inmediato (`UndeclaredToolForAgent`), y cada ejecución real vuelve a pasar por `GatekeeperEngine`.
4. **Proveedores compatibles (`provider`):** `"deep_seek"`, `"anthropic"`, `"open_ai"`, `"gemini"`, `"ollama"`. Las credenciales nunca se guardan en texto plano; se referencian mediante alias almacenados en la bóveda DPAPI (`miku secret set`).

---

## 3. Adaptadores de agentes nativos y externos
- **Claude Code:** Hook stdio fail-open (`<= 15 ms` timeout, reversible vía [Instalador de Hooks](../user/hooks-installer.md)).
- **Cursor IDE:** Integración por servidor MCP local (`miku-mcp`) sobre loopback `127.0.0.1:47410` o stdio JSON-RPC `2024-11-05`.
- **Antigravity / otros clientes:** Observación pasiva de sesión con resumen de contexto y compuertas de aprobación móvil vía `miku-link`.

Ver también: [Diseño técnico de secretos (F-08)](../design/F-08.md) y [Seguridad integral](../../SECURITY.md).
