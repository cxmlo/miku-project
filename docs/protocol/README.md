# Protocolo `miku-link v1` (WebSocket JSON)

`miku-link v1` es el contrato único de mensajes entre el daemon (`miku-gateway`), los adaptadores de CLI/IDE, la UI WebView2 (`https://miku.local`) y el cliente móvil Android ([especificación móvil](ANDROID_LINK.md), [paridad Android](../architecture/ANDROID_PARITY.md)).

---

## 1. Transporte y seguridad de enlace
- **Loopback local obligatorio por defecto:** `ws://127.0.0.1:47400/link`. Intentar enlazar a `0.0.0.0` sin túnel cifrado ni token falla de forma cerrada (`GatewayConfig::validate_bind_policy`).
- **Modo remoto fuera de casa:** `wss://<nodo>.ts.net:47400/ws` sobre Tailscale Mesh + pinning TLS SHA-256 ([guía Tailscale](../user/tailscale-tunnel.md)).
- **Validación estricta de tramas:** deserialización pura en `miku-protocol` con `#[serde(deny_unknown_fields)]`, límites `MAX_FRAME_BYTES` y `MAX_AUTH_FRAME_BYTES`, y comparación de tokens en tiempo constante (`constant_time_token_eq`).

---

## 2. Variantes del protocolo (`MikuLinkMessageV1`)

Cada sobre incluye versión (`"v": 1`), identificador único (`"id"`) y objeto etiquetado `"msg"` con `"type"` en `snake_case` + `"data"`.

### Handshake autenticado (`hello` -> `hello_ack`)
```json miku-link
{
  "v": 1,
  "id": "frame-001",
  "msg": {
    "type": "hello",
    "data": {
      "client": "miku-android-v1",
      "token": "ephemeral-32byte-hex-session-token",
      "capabilities": ["observe", "approve_low_medium"]
    }
  }
}
```

### Heartbeat (`ping` / `pong`)
```json miku-link
{
  "v": 1,
  "id": "frame-002",
  "msg": {
    "type": "ping",
    "data": {
      "ts": 1763000100
    }
  }
}
```

### Solicitud de permiso y decisión biométrica (`permission_request` -> `permission_decision`)
Las acciones clasificadas como `Critical` o `High` exigen `biometric_verified = true` en el cliente móvil y respetan el TTL configurado o bloqueo `pc_only`:

```json miku-link
{
  "v": 1,
  "id": "frame-003",
  "msg": {
    "type": "permission_request",
    "data": {
      "req_hash": "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08",
      "agent": "claude-code",
      "capability": "fs_write",
      "target": "src/main.rs",
      "ttl_sec": 30,
      "pc_only": false
    }
  }
}
```

```json miku-link
{
  "v": 1,
  "id": "frame-004",
  "reply_to": "frame-003",
  "msg": {
    "type": "permission_decision",
    "data": {
      "req_hash": "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08",
      "decision": "allow",
      "biometric_verified": true
    }
  }
}
```

---

## 3. Referencias relacionadas
- [Contratos Android / Tailscale / Biometría](ANDROID_LINK.md)
- [Matriz de Paridad Android](../architecture/ANDROID_PARITY.md)
- [Política de Seguridad (Sección 8)](../../SECURITY.md)
