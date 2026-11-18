# Miku-Link v1 · Emparejamiento LAN por QR, TLS Pinning e Invalidación («Pair Again»)

- **Módulo:** `miku/crates/miku-gateway/src/lan_pairing/` (`mod.rs`, `qr_pairing.rs`, `session_revocation.rs`, `tests.rs`)
- **Estado por defecto:** Desactivado (`enabled_by_user = false`, solo loopback `127.0.0.1`, `0` listeners LAN y `0` sobrecoste).

## Seguridad de Emparejamiento (Paso 4.8 · `F-17`)
1. **QR de un solo uso (`<= 60 s` TTL):** `PairingUriPayload` (`miku-link://pair?v=1&host=...&port=...&nonce=...&pin=sha256:...&exp=...`) + tarjeta SVG QR sin dependencias (`render_pairing_qr_svg`).
2. **TLS Pinning SHA-256:** `compute_cert_spki_pin_sha256` + verificación en tiempo constante (`constant_time_token_eq`).
3. **Anti-fuerza bruta:** Tras `3` fallos consecutivos el QR queda invalidado inmediatamente (`QrInvalidatedByRateLimit`).
4. **«Emparejar de nuevo» (`pair_again_revoke_all`):** Limpia todos los tokens móviles emitidos, incrementa `epoch` y dispara `watch::Sender<u64>` para cerrar al instante todas las sesiones WebSocket móviles abiertas.
