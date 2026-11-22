# Cambios

## 0.3.5

- **macOS**: las letras con acento (Á, é, ñ…) ya no salen como «A□». macOS entrega los
  nombres con el acento separado de la letra; ahora se unen antes de mostrarlos en
  archivos, carpetas, rutas, volúmenes (incluido el disco del instalador) y usuarios.

## 0.3.4

- **Windows**: las rutas se ven como `C:\...` (sin el prefijo `\\?\`) y la barra de ruta
  muestra `C: › Usuarios › ...`.
- **Windows**: sin columnas de dueño y permisos Unix; el panel indica «Solo lectura» u
  «Oculto» cuando aplica. El CSV tampoco las incluye.
- **Cada sistema con su vocabulario**: «administrador» en Windows y «root» en Linux; los
  ajustes solo muestran lo que existe en ese sistema (nada de /proc ni tmpfs en Windows;
  volúmenes y memoria virtual en macOS); papelera de reciclaje, disco del sistema, etc.

## 0.3.3

- **Windows**: la ventana se dibuja con Direct3D 12 (o Vulkan) en lugar de OpenGL, que falta
  en muchos equipos y hacía que el `.exe` no abriera. Si aun así falla, se reabre sola con
  OpenGL.
- Si la app no puede arrancar o se cierra por un error, muestra un aviso con el motivo y
  guarda los detalles en `crash.log` (en Windows: `%APPDATA%\arbolview\crash.log`).

## 0.3.2

- Nuevos paquetes para **Linux ARM64** (AppImage aarch64) y **Windows ARM64** (zip).
- macOS sigue en un único `.dmg` universal: Apple Silicon e Intel.

## 0.3.1

- Publicación automática: la compilación de macOS usa la máquina actual de GitHub
  (la anterior se había retirado y bloqueaba la publicación de la versión).

## 0.3.0

- Linux, Windows y macOS (Intel y Apple Silicon) con un mismo código.
- Informes en PDF, HTML, CSV y JSON.
- Línea de comandos: `scan`, `history`, `schedule`, con códigos de salida para scripts.
- Historial con gráfica y escaneos programados (systemd, launchd, Programador de tareas).
- Papelera con deshacer, duplicados, limpieza rápida, filtros avanzados, comparar carpetas.
- Modo root / administrador, columna de permisos, 4 idiomas y 6 temas.
- Paquetes: AppImage (Linux), zip con `.exe` (Windows), `.dmg` universal (macOS).
