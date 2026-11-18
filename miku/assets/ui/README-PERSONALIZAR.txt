Miku Desktop — Carpeta Personalizable de Usuario
=================================================

Estructura modular: shared/ (comun), island/ (pildora), settings/ (ventana 1080x780).
Edita o reemplaza solo los archivos que quieras personalizar (sin recompilar miku.exe):

- shared/theme.css                 -> Colores, bordes, brillos, tipografia y radios.
- shared/mascot/mascot.config.json -> Paleta, ojo, coletas, accesorios y presets del muneco.
- shared/mascot/mascot.js          -> Reemplazo completo del avatar (window.MikuMascot).
- shared/audio/sounds/manifest.json-> Mapea sonidos propios (.wav/.mp3/.ogg) por evento.
- shared/locales/es.json           -> Sobrescribe textos de la interfaz.

Atajos globales por defecto:
- Alt + Space           -> Abrir / activar dictado de voz (PTT)
- Ctrl + Alt + Shift + K-> Kill Switch de emergencia (pausa o reanuda agentes)
