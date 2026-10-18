Aplicación de escritorio para copiar carpetas y ficheros de OneDrive a tu equipo. Incluye rclone.

## Descargas

- **macOS (Apple Silicon: M1 y posteriores):** `OneDrive-Downloader-<versión>-macos-arm64.zip`
- **macOS (Intel):** `OneDrive-Downloader-<versión>-macos-x86_64.zip`
- **Windows (64 bits):** `OneDrive-Downloader-<versión>-windows-x86_64.zip`

## Primer arranque

La aplicación no está firmada con un certificado de desarrollador, así que el sistema avisará la primera vez.

- **macOS:** descomprime, mueve `Copiar de OneDrive.app` a Aplicaciones y ábrela con clic derecho y **Abrir**. Si macOS dice que está dañada, ejecuta en Terminal: `xattr -dr com.apple.quarantine "/Applications/Copiar de OneDrive.app"`.
- **Windows:** descomprime la carpeta completa (`onedrive-copier.exe` necesita `rclone.exe` a su lado) y ejecuta `onedrive-copier.exe`. Si aparece SmartScreen, pulsa **Más información** y **Ejecutar de todas formas**.

## Qué hace

Inicia sesión en OneDrive con permiso de solo lectura, permite buscar y ordenar carpetas, marcar carpetas y ficheros (con selección en cascada) y copiarlos a la carpeta que elijas. No borra nada y por defecto no sobrescribe.
