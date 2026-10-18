# Copiar de OneDrive

Aplicación de escritorio muy sencilla (Rust + egui) para macOS y Windows. Permite iniciar sesión en OneDrive,
marcar las carpetas que se quieran y copiarlas a la carpeta de destino que se elija. El trabajo pesado lo hace
[rclone](https://rclone.org), que se usa como motor.

## Qué hace

1. **Iniciar sesión** en tu cuenta de Microsoft (se abre el navegador). Solo se pide permiso de lectura.
2. **Elegir carpetas y ficheros** en un árbol que se carga bajo demanda y muestra carpetas **y ficheros**, con fecha y tamaño.
3. **Elegir destino** y pulsar **Copiar**: progreso, velocidad, tiempo restante y errores en pantalla. Se puede cancelar.

### Selección

- Marcar una carpeta marca todo lo que contiene (subcarpetas y ficheros). Una casilla a medias indica que solo
  hay una parte seleccionada.
- Dentro de una carpeta marcada se puede desmarcar lo que sobre (carpetas o ficheros): se copia todo salvo eso.
- También se pueden marcar ficheros sueltos. Los ficheros sueltos se copian a una carpeta con el nombre de la
  carpeta de origen, para que no se mezclen entre sí.
- El panel «Selección» resume lo que se va a copiar. «Calcular tamaño» muestra cuántos ficheros y cuántos GB son.

### Búsqueda y orden

- La caja de búsqueda encuentra carpetas en **todo** OneDrive (ignora mayúsculas y acentos; varias palabras, en
  cualquier orden). La primera búsqueda indexa las carpetas en segundo plano (unos 1,5 minutos con ~1.000 carpetas)
  y ya se puede buscar mientras avanza. «Abrir» lleva al sitio de esa carpeta en el árbol.
- Orden: por nombre (A-Z o Z-A, con números en orden natural: «2» antes que «10»), por fecha de modificación
  (más recientes o más antiguas primero) o por tamaño (para ficheros; las carpetas no tienen tamaño sin calcularlo).
  Las carpetas siempre van antes que los ficheros.

### Descargas simultáneas

rclone descarga por defecto 4 ficheros a la vez y trocea cada fichero grande (más de 256 MiB) en 4 flujos. La app
deja ajustar el número de ficheros simultáneos (1 a 16). Más no siempre acelera: OneDrive limita el ritmo por
cuenta y en las pruebas hechas con esta línea 3 flujos dieron ~4,3 MB/s frente a ~2,5 MB/s de uno solo.

## Garantías

- Solo lee de OneDrive: la sesión se autoriza con permisos de lectura y la app solo ejecuta `lsjson`, `copy` y el
  asistente `config` (que únicamente escribe su propio fichero de sesión).
- No borra nada, ni en OneDrive ni en el destino (`rclone copy` no elimina).
- Por defecto no sobrescribe: los ficheros que ya existen en el destino se omiten (`--ignore-existing`).
  Repetir una copia interrumpida continúa donde se quedó.
- rclone verifica cada fichero al terminar de copiarlo.
- Cada carpeta se copia dentro del destino con su mismo nombre.

## Requisitos

- [rclone](https://rclone.org/downloads/): `brew install rclone` (macOS) o `winget install Rclone.Rclone` (Windows).
  La app lo busca junto a su propio ejecutable, en el `PATH` y en las rutas habituales. También se puede copiar
  el ejecutable de rclone al lado de la app para llevarla autocontenida.
- Para compilar: [Rust](https://rustup.rs) (edición 2024).

## Descargas

Los binarios para macOS (Apple Silicon e Intel) y Windows están en la pestaña **Releases** del repositorio. Cada
zip lleva rclone dentro. Como la aplicación no está firmada, el sistema avisa la primera vez; las instrucciones están
en las notas de cada release.

### Publicar una release

El workflow `.github/workflows/release.yml` compila y prueba la aplicación en macOS (arm64 e Intel) y Windows. Al
subir una etiqueta `v*` crea la release con los tres zips:

```bash
git tag v0.1.0
git push origin v0.1.0
```

La etiqueta debe coincidir con la `version` de `Cargo.toml`. En los pull requests y al lanzarlo a mano
(Actions, «Run workflow») compila y sube los zips como artefactos, sin publicar nada.

## Compilar y ejecutar

```bash
cargo run --release
```

El ejecutable queda en `target/release/onedrive-copier` (`onedrive-copier.exe` en Windows). Compila en cada sistema
operativo para el que lo quieras: macOS en un Mac y Windows en un Windows.

Si en macOS el enlazado falla con `tapi error: malformed file` (el SDK de las Command Line Tools es más nuevo que su
enlazador), compila con un SDK anterior:

```bash
SDKROOT=/Library/Developer/CommandLineTools/SDKs/MacOSX26.5.sdk cargo run --release
```

En macOS se puede generar una aplicación `.app` (incluye rclone si está instalado):

```bash
scripts/bundle-macos.sh
```

El resultado queda en `dist/Copiar de OneDrive.app`. No está firmada: si se descarga en otro Mac, macOS pedirá
confirmación la primera vez (clic derecho, Abrir).

## Icono

La imagen original está en `assets/icon-source.png`. `scripts/make-icons.sh` limpia las salpicaduras
semitransparentes del recorte y genera `assets/AppIcon.icns` (macOS), `assets/icon.ico` (Windows) y
`assets/icon-256.png` (ventana). Necesita ImageMagick (`magick`) y `iconutil` (macOS). La `.app` usa el `.icns`; el
`.exe` de Windows incrusta el `.ico` al compilar en Windows (si no se puede, la compilación avisa y la ventana
sigue mostrando el icono).

## Dónde guarda la sesión

Una configuración propia de rclone, separada de la que tengas tú:

- macOS: `~/Library/Application Support/onedrive-copier/rclone.conf`
- Windows: `%APPDATA%\onedrive-copier\rclone.conf`

Contiene el token de acceso en texto plano (igual que rclone). «Cerrar sesión» borra ese fichero. Para revocar
además el permiso en Microsoft: <https://account.live.com/consent/Manage>.

Variables opcionales: `ONEDRIVE_COPIER_CONF` (otra ruta de configuración) y `ONEDRIVE_COPIER_RCLONE` (ruta de rclone).

## Pruebas

```bash
cargo test --release
```

Las pruebas contra un OneDrive real están marcadas como ignoradas. Hace falta una sesión ya iniciada
(`ONEDRIVE_COPIER_CONF`), el camino de una carpeta pequeña de OneDrive con al menos 2 ficheros y sin subcarpetas
(`ONEDRIVE_COPIER_TEST_FOLDER`) y una carpeta de destino local (`ONEDRIVE_COPIER_TEST_DEST`):

```bash
cargo test --release -- --ignored live_list_index_size_and_copy
cargo test --release -- --ignored live_index_streams
```
