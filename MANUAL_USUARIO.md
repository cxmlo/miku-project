# Manual de Usuario y Guía de Pruebas — Miku Desktop Orchestrator (Demo Actual)

> **Estado actual del proyecto:** Binario Rust nativo (`miku.exe`) + Dynamic Island WebView2 funcional como **demo local interactiva**.  
> **Transparencia de estado:** Todos los pasos de **Fase 7 (`7.1..7.5`) permanecen en `PENDIENTE-LOCAL`** hasta que tú pruebes la app en tu escritorio físico y confirmes que el diseño, el audio y los flujos responden como esperas. Las mejoras de diseño 1:1 respecto a `cxmlo/miku` quedan propuestas en la **Fase 8 (`8.1..8.6`)** esperando tu visto bueno.

---

## 1. Requisitos e inicio rápido

El ejecutable compilado en Release ya existe en el repositorio:

- **Binario principal:** `miku/target/release/miku.exe` (`2,848,256` bytes / `2.72 MB`)
- **Runtime necesario:** Microsoft Edge WebView2 Evergreen Runtime (detectado en tu equipo: `157.0.4341.46`)
- **Carpeta de datos y personalización:** `%LOCALAPPDATA%\Miku\`

### Modos de ejecución (`miku.exe --help`)

Desde PowerShell en la raíz `miku-project`:

| Comando | Qué hace |
| :--- | :--- |
| `.\miku\target\release\miku.exe` | Inicia el **modo residente completo**: Daemon HTTP/WS (`127.0.0.1:47400`), icono en la bandeja del sistema (System Tray), atajos globales y ventana Dynamic Island arriba al centro (`y=0`). |
| `.\miku\target\release\miku.exe serve` | Alias explícito del modo residente de escritorio (idéntico a ejecutar `miku.exe` sin argumentos). |
| `.\miku\target\release\miku.exe serve --headless` | Arranca solo el servidor HTTP/WebSocket local sin abrir ventanas WebView2. |
| `.\miku\target\release\miku.exe ui` | Abre únicamente la ventana Dynamic Island conectada a `127.0.0.1:47400`. |
| `.\miku\target\release\miku.exe demo` | Ejecuta una sesión de demostración aislada en sandbox sin modificar archivos ni secretos en disco. |
| `.\miku\target\release\miku.exe doctor [--json]` | Comprueba puerto `47400`, WebView2, huérfanos, contrato `.miku.toml` y presupuestos. |
| `.\miku\target\release\miku.exe ci-bench --runs 5 --json` | Mide latencia de pastilla, arranque en frío, RAM privada y procesos huérfanos. |
| `.\miku\target\release\miku.exe secret ...` | Guarda, consulta, lista o elimina claves API en Windows Credential Manager (DPAPI). |
| `.\miku\target\release\miku.exe install ...` | Registra acceso o arranque automático en Windows. |

---

## 2. Qué funciona HOY en la demo

### A. Núcleo Rust, Daemon y Seguridad
1. **Gateway HTTP/WebSocket loopback (`127.0.0.1:47400`):** sirve la interfaz WebView2, recibe mensajes en tiempo real por WebSocket y expone rutas reales como `GET /api/state`, `GET /api/bootstrap`, `POST /api/chat`, `POST /api/settings` y `POST /api/gatekeeper/kill_switch`.
2. **Almacén de claves con DPAPI (`miku secret`):** las claves de API (por ejemplo `deepseek-main`) se cifran con Windows Credential Manager / DPAPI; no se guardan en texto plano dentro del JSON de ajustes.
3. **Cliente LLM HTTPS nativo (`winhttp_https.rs`):** las conversaciones en el chat contra el proveedor configurado (como DeepSeek) se envían directamente desde Rust sin dependencias TLS pesadas.
4. **System Tray y atajos globales (`win32_tray.rs`, `desktop_daemon.rs`):**
   - `Alt + Space`: despertar / activar modo PTT.
   - `Ctrl + Alt + Shift + K`: **Kill Switch** de emergencia para pausar o reanudar agentes y herramientas.
   - Menú en System Tray: alternar visibilidad de la Isla, abrir carpeta `%LOCALAPPDATA%\Miku\ui\`, abrir configuración, activar Kill Switch, ejecutar `doctor` y cerrar limpiamente.
5. **Gatekeeper y `.miku.toml`:** valida políticas y confirmaciones sensibles antes de ejecutar comandos críticos.
6. **Emparejamiento QR Miku-Link (`qr_codec`):** genera matrices QR reales ISO/IEC 18004 en SVG/Canvas desde Rust.

### B. Interfaz Dynamic Island y Mascota Personalizable
1. **Marco Win32 transparente real (`y=0`):** renderizado sin bordes usando composición DWM (`WS_EX_NOREDIRECTIONBITMAP` + `DwmExtendFrameIntoClientArea` + fondo WebView2 transparente) sin robo de foco al mostrarse como pastilla.
2. **Mascota original sin coletas Vocaloid por defecto:** `mascot/mascot.config.json` usa `schemaVersion: 2`, `defaultColor: "#ffffff"` y `enableTwintails: false`. Los accesorios laterales quedan desactivados salvo que los actives manualmente en Guardarropa.
3. **Escenas actualmente implementadas:**
   - **Compacta (pastilla):** indicador de estado, avatar reactivo y expansión al hacer clic.
   - **Inicio (Overview):** tarjeta de estado del agente activo + paginador derecho de utilidades.
   - **Chat:** selección de agente, envío de prompts reales a `/api/chat`, respuesta con formato Markdown y botón de copia.
   - **Guardarropa y Galería:** **15 estilos/atuendos** (`Ninguno` + 14 accesorios), selector de color corporal, interruptor opcional de accesorios laterales y **Galería de 25 expresiones** del muñeco.
   - **Miku-Link QR:** generación de nuevo QR, copia de URL de enlace local y desvinculación.
   - **Ajustes (embebidos temporalmente en la isla):** 6 pestañas (*Muñeco*, *API Keys*, *Gatekeeper*, *Módulos*, *Voz/Idioma*, *Sistema*).
   - **Tarjetas de flujo:** Gatekeeper (`approve`), preguntas (`question`), errores (`error_card`), carga de archivos, descarga MCP y actualizaciones.
4. **Audio UI (`sound-fx.js`):** sonidos sintéticos WebAudio (`click`, `pop`, `notification`, `success`, `alert`, `update`, `chime`, `error`) desacoplados del TTS, más soporte para audios propios `.wav`, `.mp3` u `.ogg` vía `manifest.json`.

---

## 3. Cómo está organizada la UI y cómo personalizarla sin recompilar

Los archivos base viven en `miku/assets/ui/`, pero **antes de cargar un recurso**, `miku.exe` revisa si existe una copia en:

```text
%LOCALAPPDATA%\Miku\ui\
```

Si copias un archivo allí manteniendo su ruta relativa, puedes cambiar diseño, mascota o sonidos **sin recompilar Rust**:

| Archivo (relativo a `miku/assets/ui/` o `%LOCALAPPDATA%\Miku\ui\`) | Responsabilidad |
| :--- | :--- |
| `theme.css` | Variables CSS de colores, bordes, sombras, fuentes Nunito locales (`/fonts/*.woff2`) y radios. |
| `island.css` | Geometría y disposición de la pastilla, tarjetas, chat, botones y responsividad de escenas. |
| `index.html` / `app.js` | Estructura HTML raíz de la isla, ciclo de vida, cambio de escenas y puente con `MikuMascot`. |
| `mascot/mascot.config.json` | Configuración por defecto de la mascota (`defaultColor`, `enableTwintails`, lista de `outfits`). |
| `mascot/mascot.js` | Motor del muñeco. **Contrato sustituible:** cualquier implementación nueva (SVG, Canvas 2D, WebGL o sprites) que exponga `window.MikuMascot` (`mount`, `unmount`, `setState`, `emote`, `setOutfit`, `setColor`, `lookAt`, `setEatProgress`, `renderFrame`, `getCapabilities`) y `window.buddy` reemplaza al muñeco sin tocar el resto de la app. |
| `mascot.html` | Banco de pruebas aislado para editar/probar el muñeco directamente en el navegador. |
| `audio/sound-fx.js` + `audio/sounds/manifest.json` | Efectos de interfaz y mapeo a archivos `.wav`/`.mp3`/`.ogg` personalizados por clave de evento. |
| `scenes/renderers.js`, `scenes/chat.js`, `scenes/settings.js` | Plantillas y lógica de cada escena (Overview, Chat, Guardarropa, Galería, Ajustes, QR). |
| `core/state.js`, `core/bridge.js`, `core/components.js`, `core/qr.js` | Estado compartido, WebSocket/HTTP con `127.0.0.1:47400`, iconos/componentes y renderizado QR. |
| `locales/es.json`, `locales/en.json` | Diccionario de textos de la interfaz. |

> **Nota importante sobre el color azul/cian actual (`#3B9EFF`):**  
> En este equipo existe actualmente un override previo en `%LOCALAPPDATA%\Miku\ui\theme.css` con `--ac: #3B9EFF` y `--island-bg: #050609` (igual que el `theme.css` base actual). Si quieres probar colores monocromáticos antes de que ejecutemos la Fase 8, puedes editar o renombrar `%LOCALAPPDATA%\Miku\ui\theme.css`.

---

## 4. Qué hacer para probar todo paso a paso (Checklist de Usuario)

### Paso 1 — Verificar salud del sistema y métricas base
Ejecuta en PowerShell dentro de `c:\Users\cxmluwu\Desktop\miku-project`:

```powershell
.\miku\target\release\miku.exe doctor --json
.\miku\target\release\miku.exe ci-bench --runs 5 --json
.\miku\target\release\miku.exe demo
```

- **Qué debes ver:** `overall_status: "PASS"` (`5` checks verdes), `passed: true` en `ci-bench` (`max_orphans: 0`) y salida de sesión `demo` sin errores.

### Paso 2 — Verificar tu clave API (opcional, para probar Chat LLM real)
```powershell
.\miku\target\release\miku.exe secret list
```
Si aún no has guardado tu API key:
```powershell
.\miku\target\release\miku.exe secret set deepseek-main --provider deepseek --model deepseek-chat
```
*(El comando ocultará la escritura en consola usando la API Win32 de DPAPI/Credential Manager).*

### Paso 3 — Lanzar Miku en el escritorio físico
```powershell
.\miku\target\release\miku.exe
```

Comprueba visualmente en tu pantalla:
1. **Pastilla superior (`y=0`):** flota centrada arriba, con esquinas/orejas integradas y **fondo transparente alrededor** (sin rectángulo negro sólido cubriendo tu escritorio).
2. **Tray icon:** aparece el icono en la bandeja del sistema (junto al reloj). Haz clic derecho para verificar el menú (*Alternar Isla*, *Carpeta UI personalizada*, *Kill Switch*, *Doctor*, *Salir*).

### Paso 4 — Probar interacciones de la Isla
1. **Expandir la pastilla:** haz clic sobre la pastilla compacta para abrir **Inicio**.
2. **Guardarropa y Galería (25 expresiones):**
   - Entra en **Guardarropa** (icono de camiseta o tarjeta rápida).
   - Cambia entre los atuendos (*Ninguno*, *Corona*, *Gorro*, *Navidad*, *Lentes*, etc.) y verifica que la mascota cambia al instante **sin coletas azules** por defecto.
   - Entra en **Galería (25)** y pulsa distintos estados (`idle`, `happy`, `thinking`, `angry`, `eat`, `sleeping`, etc.) para comprobar que cada animación responde.
3. **Audio (`SoundFX`):**
   - Ve a **Ajustes > Voz / Idioma**.
   - Asegúrate de que los efectos UI estén activados y sube el deslizador de volumen.
   - Pulsa los botones de prueba de sonido (*Click*, *Notificación*, *Éxito*, *Alerta*, *Error*) y confirma que se escuchan con claridad y sin interferencias de voz TTS.
4. **Chat LLM real:**
   - Entra en **Chat**, escribe un mensaje (ej. `"Hola, dime qué sabes hacer"`), envía y verifica que llega la respuesta real desde tu proveedor configurado en DPAPI.
5. **Miku-Link QR:**
   - Pulsa el icono de teléfono para ver el QR generado localmente, probar **Nuevo QR** y **Copiar URL**.
6. **Atajos globales:**
   - Fuera de la ventana de Miku, pulsa `Alt + Space` para comprobar el despertar PTT.
   - Pulsa `Ctrl + Alt + Shift + K` para activar el **Kill Switch** y pulsa de nuevo para desactivarlo.

### Paso 5 — Probar personalización sin recompilar
1. Abre `%LOCALAPPDATA%\Miku\ui\mascot\mascot.config.json`.
2. Cambia temporalmente `enableTwintails` o `defaultColor`, guarda el archivo y recarga o reinicia la Isla.
3. Confirma que el cambio aplica inmediatamente sin recompilar `miku.exe`.

### Paso 6 — Confirmar cierre sin procesos huérfanos
Sal desde el menú del System Tray (*Salir*) o cierra la consola, y luego ejecuta:

```powershell
Get-Process miku, msedgewebview2 -ErrorAction SilentlyContinue
```

- **Qué debes ver:** `miku` cerrado por completo y cero procesos `msedgewebview2.exe` huérfanos dejados por Miku.

---

## 5. Qué queda pendiente / Limitaciones conocidas (Plan Fase 8)

Durante tu primera captura en vivo y nuestra auditoría contra `https://github.com/cxmlo/miku` detectamos **6 brechas claras** entre la demo actual y el diseño objetivo que pediste. Todas están planificadas en la **Fase 8 (`8.1..8.6`)** y **NO se modificarán hasta recibir tu visto bueno**:

1. **Ajustes amontonados dentro de la Isla (`8.1` + `8.6`):** hoy los 6 bloques de configuración viven dentro de la propia pastilla; en `cxmlo/miku` al pulsar *Ajustes...* se abre una **ventana independiente `Configuración · Miku` (`1080x780`)** con sidebar agrupada (`BASE`, `INTELIGENCIA`, `ASISTENCIA`, `SISTEMA`), banner de partículas y tarjetas de permisos (*Toda mi PC* / *Sandbox* / *Restringido*).
2. **Estética cian vs monocromo puro (`8.2`):** el tema actual aún usa bordes/resplandores `#3B9EFF`, textos extra en el header (`Inicio`, `47400 OK`, botón camiseta, `X`) y la etiqueta `miku-local`, en vez del monocromo puro (`#000` / `#0e0f11` / `#17181b` / `#f5f5f7`) y barra limpia (`[Home][Chat][+] | [Phone][Gear][Speaker]`).
3. **Recorte inferior / caja negra en Página 2 de Inicio (`8.2` + `8.5`):** al cambiar a la página del reproductor ocurre solapamiento/recorte en el borde inferior del HWND y hay 3 páginas en lugar de las 2 páginas limpias (*Agentes* `[GitHub][Antigravity]` + *Reproductor OS GSMTC*).
4. **Guardarropa con SVG en vez de miniaturas Canvas (`8.3`):** actualmente las fichas del guardarropa usan iconos vectoriales estáticos en lugar de miniaturas circulares en vivo del propio muñeco (`MikuMascot.avtHtml`).
5. **Tracking del mouse limitado dentro de la ventana (`8.4`):** cuando el puntero sale de la pastilla hacia el resto del escritorio o ventana de otra app, Miku deja de seguirlo porque falta el puente `Win32 GetCursorPos` escalado por DPI.
6. **Quick Popover dentro de la Isla (`8.5`):** falta separar el popover rápido del engranaje (*Sonido y voz*, volumen, *Abrir al pasar el cursor*, chips *Core / API / Miku-Link* y botón *Ajustes...*).
