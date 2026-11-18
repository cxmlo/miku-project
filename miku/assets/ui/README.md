# Miku Dynamic Island — Arquitectura Modular Dual-Ventana

Cada archivo dentro de `miku/assets/ui/` es independiente y personalizable sin recompilar `miku.exe`.

## Estructura modular (`shared/`, `island/`, `settings/`)

- `shared/`: Recursos compartidos entre ambas ventanas
  - `theme.css`: Tokens de color, radios, tipografía y resplandores
  - `mascot/mascot.js` y `mascot/mascot.config.json`: Avatar procedural (`window.MikuMascot` + `window.buddy`)
  - `audio/sound-fx.js` y `audio/sounds/manifest.json`: Efectos sonoros
  - `core/`: `bridge.js`, `state.js`, `components.js`, `qr.js`
  - `locales/`: `es.json`, `en.json`
  - `fonts/`: Fuente `Nunito` offline (cero CDN)
- `island/`: Dynamic Island flotante superior (`index.html`, `mascot.html`, `island.css`, `app.js`, `scenes/*.js`)
- `settings/`: Ventana independiente **Configuración · Miku** `1080×780` (`miku_settings.html`, `settings.css`, `settings_app.js`)

## Sobrescrituras del usuario (`%LOCALAPPDATA%\Miku\ui\`)

Copia únicamente el archivo que quieras personalizar dentro de `%LOCALAPPDATA%\Miku\ui\` manteniendo la ruta modular (`shared/...`, `island/...`, `settings/...`). El servidor también resuelve rutas planas heredadas (`theme.css`, `mascot/mascot.config.json`, etc.) por compatibilidad.
