# Acceso Remoto Fuera de Casa con Tailscale Mesh (ADR-013 · F-17)

Por decisión de arquitectura (**ADR-013**, reemplazando Cloudflare Tunnel por una solución 100 % gratuita sin dependencias comerciales de proxy HTTP):

## Por qué Tailscale
- **Gratis desde el primer día** en cuenta personal hasta 100 dispositivos / 3 usuarios.
- **0 puertos abiertos en tu router**: malla WireGuard cifrada punto a punto (`100.64.0.0/10` y subdominios `*.ts.net`).
- **Doble autenticación**: peer autorizado de tu Tailnet + **token efímero de Miku** verificado en el primer frame (`MikuLinkMessageV1::Hello`) antes de tocar el `EventBus` + **TLS SHA-256 pinning**.
- **Apagado por defecto**: solo escucha en modo «Fuera de casa» y se corta instantáneamente con el Kill Switch (`Ctrl+Alt+Shift+K` o barra roja en Android).

## Pasos de configuración (2 minutos)
1. Instala **Tailscale** en Windows 11 ([tailscale.com/download](https://tailscale.com/download)) y en tu móvil Android desde Google Play.
2. Inicia sesión con tu cuenta en ambos dispositivos (`tailscale up`).
3. Verifica con Miku Doctor:
   ```powershell
   cargo run --manifest-path miku/Cargo.toml -p miku-cli -- doctor --json
   ```
4. Activa el modo **«Fuera de casa»** en la Island -> Miku detectará tu URL `wss://<nodo>.ts.net:47400/ws`, rotará un token efímero y aceptará conexiones **únicamente** desde tu Tailnet.
