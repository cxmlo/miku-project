# Despertador Push sin PII (Paso 4.11 · Opcional en v1.0)

El despertador de **Miku-Link** es 100 % opcional (apagado por defecto) y opera sin bases de datos ni almacenamiento de historial.

## Garantía Cero PII
Cada señal transmitida contiene únicamente **3 campos**:
```json
{
  "agent_name": "claude-code",
  "state": "needs_permission",
  "event_ts": 1763000100
}
```
Jamás incluye rutas de archivos, fragmentos de código, diffs, prompts ni claves API. Tanto `miku-tunnel::push_wake` en Rust como `tools/miku-push-wake/index.mjs` validan el esquema con `deny_unknown_fields` / rechazo explícito en primer nivel.

## Ejecución en 1 comando (malla privada Tailscale o loopback)
```powershell
node tools/miku-push-wake/index.mjs
```
Si no lo activas, la conexión directa autenticada por Tailscale del Paso 4.10 sigue funcionando como canal principal.
