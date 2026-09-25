# gitsync

[English](README.md) | Español

[![CI](https://github.com/cristobaltormo/git-sync/actions/workflows/ci.yml/badge.svg)](https://github.com/cristobaltormo/git-sync/actions/workflows/ci.yml)
[![Licencia](https://img.shields.io/badge/licencia-Apache--2.0-blue)](LICENSE)

Replica tus repositorios de un servidor git propio en GitHub y los mantiene al
día. Un push llega a GitHub en pocos segundos. Renombrar un repo, hacerlo
público, cambiar su descripción, archivarlo o borrarlo también se detecta igual
de rápido.

gitsync es un único binario estático. En reposo usa unos 11 MB de RAM en Linux y
casi nada de CPU, no necesita cron ni runtime, solo `git`. Funciona en Linux,
macOS, Windows y Docker, y sus tests se ejecutan en los tres sistemas en cada
push.

## Servidores compatibles

| Servidor | Estado | Webhooks |
|---|---|---|
| Forgejo, Codeberg | probado | un webhook de sistema (token de admin) o uno por repo |
| Gitea | probado | uno por repo (el de sistema exige reiniciar Gitea, ver la guía) |
| GitLab, autoalojado y gitlab.com | probado | un webhook de sistema (token de admin) o uno por repo |
| Gogs | probado | uno por repo |
| GitBucket | probado | uno por repo |
| OneDev | probado | uno por repo |
| Bitbucket Cloud | probado | uno por repo, Bitbucket tiene que poder llegar a tu servidor (ver la guía) |
| Bitbucket Server y Data Center | experimental | uno por repo |

Probado significa que cada paso de la sincronización se ejecutó contra un
servidor real: Forgejo 16, Gitea 1.27, GitLab 19, Gogs 0.14, GitBucket 4.48,
OneDev 16 y Bitbucket Cloud. Bitbucket Server y Data Center se escribieron a
partir de su documentación y solo se han ejercitado contra un servidor simulado,
porque probarlos exige una licencia. Lo que puede y no puede hacer cada servidor
está en [docs/providers.md](docs/providers.md) (en inglés).

## Qué es instantáneo y qué no

Los servidores git envían un webhook cuando alguien hace push o crea o borra una
rama o etiqueta. La mayoría no envía nada cuando se renombra un repo, se hace
público o privado, se archiva, se cambia su descripción o se borra. gitsync
escucha los webhooks y además consulta el servidor periódicamente.

| Cambio en el servidor git | Se detecta por | En GitHub tras |
|---|---|---|
| push, rama nueva o borrada, etiqueta nueva | webhook | 2 a 5 segundos |
| repo nuevo | webhook o consulta periódica | 3 a 5 segundos |
| visibilidad, descripción, web, temas, rama por defecto, archivado, renombrado | consulta periódica, o webhook si el servidor lo envía | hasta 5 segundos, más lo que tarde en aplicarse |
| repo borrado | consulta periódica y una confirmación | unos 15 segundos |

La consulta periódica lista tus repos cada 5 segundos y compara el resultado con
el listado anterior. Son unos 3 KB por repo y unos milisegundos en el servidor.
No depende de lo que cada servidor actualice al editar algo: compara los campos
que se replican. Cuando algo difiere, solo se sincroniza ese repo, y si un
webhook se pierde la consulta sigue detectando el push. `poll_interval` se puede
bajar, o poner a 0 para depender solo de los webhooks.

Las latencias medidas en cada servidor están en
[docs/benchmarks.md](docs/benchmarks.md).

## Instalación

Descarga el binario para tu equipo desde la
[página de releases](https://github.com/cristobaltormo/git-sync/releases):

```
curl -L -o gitsync https://github.com/cristobaltormo/git-sync/releases/latest/download/gitsync-linux-amd64
chmod +x gitsync
```

En Windows, en PowerShell:

```
Invoke-WebRequest https://github.com/cristobaltormo/git-sync/releases/latest/download/gitsync-windows-amd64.exe -OutFile gitsync.exe
```

Hay binarios para Linux (amd64, arm64, armv7), macOS (Intel y Apple silicon) y
Windows (amd64 y arm64). Para compilarlo tú necesitas Go 1.24 o posterior:

```
go install github.com/cristobaltormo/git-sync/cmd/gitsync@latest
```

Hace falta `git` 2.32 o posterior en el equipo ([Git para Windows](https://gitforwindows.org) en Windows).

## Puesta en marcha

```
gitsync init      # hace unas preguntas y escribe config.toml
gitsync check     # prueba los dos tokens, sus permisos y cada cuenta
gitsync run --dry-run
```

`run --dry-run` registra lo que haría y no cambia nada en ninguno de los dos
lados. Cuando todo se ve bien:

```
sudo gitsync install
```

Esto crea un usuario `gitsync`, copia el binario a `/usr/local/bin` y la
configuración a `/etc/gitsync/config.toml`, y arranca un servicio de systemd. Los
logs están en `journalctl -u gitsync -f`. Tras editar la configuración,
`systemctl reload gitsync` la aplica sin reiniciar.

En Windows, ejecuta `gitsync install` desde un PowerShell abierto como
administrador. Copia el binario a `C:\Program Files\gitsync` y la configuración a
`C:\ProgramData\gitsync`, con acceso restringido a administradores, y registra
una tarea programada que arranca con Windows, se ejecuta como SYSTEM y se vuelve
a lanzar en menos de un minuto si se detiene. Si el listener de webhooks no está
en localhost, también abre ese puerto en el cortafuegos de Windows. El log es
`C:\ProgramData\gitsync\gitsync.log` y los espejos están en
`C:\ProgramData\gitsync\state`. En Windows no existe `kill -HUP`: tras editar la
configuración, ejecuta `gitsync install` otra vez, o `schtasks /End /TN gitsync`
y deja que la tarea lo arranque. `gitsync uninstall` elimina la tarea, la regla
del cortafuegos y el binario.

En macOS no hay instalador: mantén `gitsync run` en marcha con launchd, o usa
Docker.

### Tokens

En el servidor git, crea un token de acceso que pueda leer los repositorios y
gestionar sus webhooks. Si pertenece a un administrador, gitsync crea un webhook
de sistema que cubre todos los repos, incluidos los que crees después, en los
servidores que lo admiten. Si no, crea un webhook por repo.

En GitHub, un token de acceso personal clásico con los permisos `repo` y
`delete_repo`. Quita `delete_repo` si pones `on_delete` en `"archive"` o
`"ignore"`. Un token de grano fino también sirve si tiene permisos de
Administration y de escritura en Contents sobre los repos.

### Si los webhooks nunca llegan

La mayoría de servidores se niegan a llamar a direcciones de redes privadas o de
la propia máquina salvo que lo permitas. El ajuste de cada servidor está en
[docs/providers.md](docs/providers.md). Si prefieres no tocar la configuración
del servidor git, pon `hooks.mode = "none"`: todo sigue funcionando gracias a la
consulta periódica, solo que un push tarda de 4 a 8 segundos en aparecer en lugar
de 2 a 5.

## Configuración

[`examples/config.example.toml`](examples/config.example.toml) lista todas las
opciones con su valor por defecto y lo que hacen (en inglés). Estas son las que
la gente suele cambiar:

```toml
[accounts]
alice = "alice"                # propietario en el origen = propietario en GitHub
my-org = "my-github-org"       # las organizaciones también valen

[filter]
skip_private = true            # publicar solo repos públicos
topic = "github"               # o: solo repos que lleven este tema
exclude = ["scratch-*", "alice/dotfiles"]

[sync]
on_delete = "archive"          # "delete", "archive" o "ignore"
visibility = false             # no tocar nunca público/privado en GitHub
```

El filtro `topic` te permite decidir qué va a GitHub desde la propia interfaz del
servidor git: añade el tema a un repo y empieza a sincronizarse, quítalo y deja
de hacerlo. El tema en sí no se copia a GitHub.

## Cosas que conviene saber

La copia en GitHub es exacta. Las ramas y etiquetas se suben a la fuerza y se
podan, así que GitHub siempre es igual al origen y lo que hagas directamente en
la copia de GitHub se sobrescribe. Solo se replican los datos de git, más la
descripción, la web, los temas, la rama por defecto, la visibilidad y la marca de
archivado, cuando el servidor los ofrece. Pull requests, issues, wikis, releases
y objetos LFS no se replican.

Los repos nuevos se crean privados en GitHub, se suben y solo entonces se abren
si el origen es público. Un repo que es privado en el origen nunca pasa a ser
público.

Un repo que ya existe en GitHub y no fue creado por gitsync se deja como está y
se avisa en el log. Pon `adopt_existing = true` para que gitsync se haga cargo de
él, sabiendo que la copia de GitHub se sobrescribirá para igualar el origen.

El borrado es cuidadoso a propósito. Un repo que desaparece del origen solo se
trata pasados `delete_grace` segundos, y solo si preguntar directamente al
servidor por él confirma que ya no existe, porque un token que perdió acceso se
ve exactamente igual que un repo borrado. Se borran como mucho `delete_limit`
repos por hora, y un repo que pertenezca a otra persona en GitHub nunca se toca.
`on_delete = "archive"` conserva el código en su lugar.

Los renombrados se aplican al repo de GitHub in situ, así que las estrellas y los
enlaces se mantienen. Si el nombre nuevo ya está ocupado en GitHub, gitsync se
detiene y lo dice.

GitHub aplica los cambios de visibilidad despacio. Si haces un repo público y
privado otra vez en pocos segundos, aparece "a previous visibility change is
still in progress" y los push se rechazan durante un rato. gitsync lo reconoce,
espera diez segundos y lo vuelve a intentar.

GitHub rechaza los push que contienen un token que reconoce. El error queda en el
log y el repo se reintenta con esperas crecientes hasta que el secreto desaparece
del historial.

## Docker

Desde un clon de este repositorio:

```
docker build -t gitsync .
docker run -d --name gitsync --restart unless-stopped \
  -v ./config.toml:/config/config.toml:ro -v gitsync-data:/data \
  gitsync
```

Dentro de un contenedor pon `listen.host = "0.0.0.0"` y `listen.public_url` con
la dirección que usa el servidor git para llegar a él, por ejemplo
`http://gitsync:9001/hook` cuando ambos están en la misma red de Docker.
[`compose.yaml`](compose.yaml) es un punto de partida. Si el servidor git está en
otra máquina, publica el puerto (`-p 9001:9001`) y usa la dirección de esa
máquina en `public_url`. Los tokens pueden venir del entorno en lugar del
fichero: `GITSYNC_SOURCE_TOKEN`, `GITSYNC_GITHUB_TOKEN` y
`GITSYNC_WEBHOOK_SECRET`.

## Comandos

| | |
|---|---|
| `gitsync init` | escribe un fichero de configuración haciendo preguntas |
| `gitsync check` | valida la configuración, los tokens, los permisos y las cuentas |
| `gitsync run [--dry-run]` | el demonio |
| `gitsync sync [propietario/nombre ...]` | replica una vez y termina, todo o solo algunos repos |
| `gitsync status` | cada repo, a dónde va, cuándo se sincronizó por última vez, último error |
| `gitsync hooks [--force] [--remove]` | crea, reescribe o elimina los webhooks |
| `gitsync install` / `uninstall` | servicio systemd en Linux, tarea programada en Windows |

`-c fichero.toml` elige otra configuración. Sin él se prueban `./config.toml` y
después `/etc/gitsync/config.toml`. `kill -HUP` recarga la configuración.

## Uso de recursos

En reposo: alrededor de 11 MB de RAM, unos 10 ms de CPU por minuto con 20 repos y
una petición HTTP cada `poll_interval` segundos. Sincronizar es cuando `git` usa
memoria, y está configurado para que use poca. La unidad de systemd tiene un
límite duro de 512 MB que puedes cambiar. Las cifras y el método están en
[docs/benchmarks.md](docs/benchmarks.md).

El estado es un pequeño fichero JSON más un espejo bare de cada repo en
`/var/lib/gitsync` (`C:\ProgramData\gitsync\state` en Windows). Los espejos se
pueden borrar en cualquier momento y se vuelven a descargar en la siguiente
sincronización.

## Solución de problemas

`gitsync check` detecta la mayoría de problemas de configuración. Después,
`gitsync status` muestra el último error de cada repo, y `log.level = "debug"`
registra cada webhook recibido.

- Los push tardan varios segundos: el webhook no está llegando, mira "Si los
  webhooks nunca llegan".
- `already exists on GitHub and gitsync did not create it`: mira
  `adopt_existing`.
- `the token belongs to 'x' and cannot create repos under the user 'y'`: un token
  solo puede crear repos en su propia cuenta o en organizaciones a las que
  pertenece. Asigna ese propietario del origen al usuario del token o a una
  organización.
- Un repo que borraste sigue en GitHub: ejecuta `gitsync check` y busca el
  permiso `delete_repo`.

## Desarrollo

```
make test      # go vet y go test -race
make build
make release   # binarios compilados para cada sistema y sumas de control en dist/
```

Mira [docs/architecture.md](docs/architecture.md) para saber cómo está montado y
[CONTRIBUTING.md](CONTRIBUTING.md) para añadir un servidor.

## Preguntas

Usa [GitHub Discussions](https://github.com/cristobaltormo/git-sync/discussions)
para preguntas e ideas, y los issues para errores. La salida de `gitsync check` y
el log con `log.level = "debug"` (los tokens se ocultan) hacen mucho más fácil
atender un reporte.

## Créditos

La idea de este proyecto, y su primera versión, son de
[Mario Gómez](https://github.com/mariogdn), que escribió un script inicial que
subía repositorios a GitHub cada hora. gitsync se desarrolló a partir de ahí por
Cristóbal Tormo.

## Licencia

Apache License 2.0, mira [LICENSE](LICENSE) y [NOTICE](NOTICE).
