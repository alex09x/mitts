<!-- translation-source: 5326b5270d0104686a854dfcc2bbeb2fa32da1b6 -->

> [!NOTE]
> Esta es una traducción de la [documentación oficial en inglés](https://github.com/alex09x/mitts/blob/5326b5270d0104686a854dfcc2bbeb2fa32da1b6/README.md) en el commit 14af528.
> La versión canónica y más actualizada es el [README en inglés](https://github.com/alex09x/mitts/blob/main/README.md).

<p align="center">
  <img src="../site/logo.svg" width="96" height="96" alt="Mitts Build logo">
</p>

<h1 align="center">Mitts Build</h1>

<p align="center">
  <b>Trabajo de compilación, contabilizado.</b><br>
  Un sistema de compilación centrado en Rust con una caché de artefactos direccionada por contenido a nivel de máquina
  y seguimiento explícito de entradas. Lee tus archivos <code>Cargo.toml</code> y <code>Cargo.lock</code> existentes.
</p>

<p align="center">
  <a href="https://mittsbuild.com"><img src="https://img.shields.io/badge/site-mittsbuild.com-ff3b30" alt="Website"></a>
  <img src="https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-c9ced4" alt="License: MIT OR Apache-2.0">
  <img src="https://img.shields.io/badge/language-Rust-1b1d20?logo=rust" alt="Written in Rust">
  <img src="https://img.shields.io/badge/platform-Linux%20%7C%20macOS-1b1d20" alt="Platforms: Linux, macOS">
  <img src="https://img.shields.io/badge/status-pre--release-ff3b30" alt="Status: pre-release">
  <a href="https://doi.org/10.5281/zenodo.24030067"><img src="https://zenodo.org/badge/DOI/10.5281/zenodo.24030067.svg" alt="DOI: 10.5281/zenodo.24030067"></a>
</p>

<p align="center">
  <a href="#quick-start">Inicio rápido</a> ·
  <a href="#how-it-works">Cómo funciona</a> ·
  <a href="#architecture">Arquitectura</a> ·
  <a href="../docs/architecture.md">Protocolo</a> ·
  <a href="#limitations">Limitaciones</a> ·
  <a href="../ROADMAP.md">Hoja de ruta</a>
</p>

<p align="center">
  <a href="https://github.com/alex09x/mitts/blob/main/README.md">English</a> ·
  <a href="https://github.com/alex09x/mitts/blob/main/readme/README.ru.md">Русский</a> ·
  <a href="https://github.com/alex09x/mitts/blob/main/readme/README.zh-CN.md">简体中文</a> ·
  <b><a href="https://github.com/alex09x/mitts/blob/main/readme/README.es.md">Español</a></b> ·
  <a href="https://github.com/alex09x/mitts/blob/main/readme/README.pt-BR.md">Português (Brasil)</a> ·
  <a href="https://github.com/alex09x/mitts/blob/main/readme/README.ja.md">日本語</a> ·
  <a href="https://github.com/alex09x/mitts/blob/main/readme/README.de.md">Deutsch</a> ·
  <a href="https://github.com/alex09x/mitts/blob/main/readme/README.fr.md">Français</a> ·
  <a href="https://github.com/alex09x/mitts/blob/main/readme/README.ko.md">한국어</a> ·
  <a href="https://github.com/alex09x/mitts/blob/main/readme/README.ar.md">العربية</a>
</p>

---

Mitts planifica el espacio de trabajo por sí mismo e invoca a `rustc` directamente. Cada
acción de compilación obtiene una clave derivada de todas sus entradas. El resultado se
almacena una sola vez en un almacén inmutable a nivel de máquina y se restaura en cada
espacio de trabajo que lo necesite, a través de proyectos, árboles de trabajo (worktrees)
y ramas.

Si una entrada no se puede contabilizar con certeza, la acción se recompila en lugar de adivinarse.

<a id="the-problem"></a>
## El problema

La caché de compilación de Cargo reside en el directorio `target/` de cada espacio de trabajo. Cualquier operación que cree un nuevo `target/` comienza desde cero:

- un segundo clon o `git worktree` del mismo repositorio,
- cambiar a una rama que modifica `Cargo.lock` y luego volver a la anterior,
- otro proyecto que depende de los mismos crates con las mismas características (features),
- agentes de programación paralelos, cada uno trabajando en su propio worktree.

Los mismos `serde`, `syn` o `tokio` se compilan una y otra vez, y cada copia consume espacio en disco.

Las soluciones habituales tienen aristas problemáticas:

- **Un `CARGO_TARGET_DIR` compartido** serializa las compilaciones detrás de un único bloqueo, y los proyectos con diferentes conjuntos de características sobrescriben los artefactos de los demás.
- **Un wrapper del compilador como sccache** observa una invocación de `rustc` a la vez. Su clave depende de rutas absolutas a menos que estén normalizadas, y los crates que invocan al enlazador (`bin`, `proc-macro`, `dylib`) no se almacenan en caché. Cargo sigue planificando la compilación y ejecutando cada script de compilación.

Mitts toma un camino diferente: reemplaza el ejecutor de Cargo, no el compilador. Ve el grafo completo de unidades (unit graph), lo que le permite identificar cada unidad según las claves de sus dependencias, almacenar en caché las unidades de enlace y las ejecuciones de scripts de compilación, y permitir que varias compilaciones compartan un almacén de forma segura.

<a id="highlights"></a>
## Aspectos destacados

- **Reutilización entre espacios de trabajo.** Un único almacén por máquina. Una dependencia compilada para una copia de trabajo se restaura en la siguiente con las mismas entradas.
- **Claves completas.** Cadena de herramientas (toolchain), destino (target), perfil, opciones, características, contenido de fuentes, claves de dependencias y entorno rastreado. Las entradas desconocidas implican una recompilación.
- **Los manifiestos de Cargo siguen siendo el contrato.** Sin nuevos manifiestos, sin un segundo archivo de bloqueo. Los flujos de trabajo de compilación, prueba, lint, cobertura y fuentes de publicación de este repositorio se ejecutan a través de Mitts; no invocan la CLI de Cargo. La interoperabilidad externa con Cargo sigue estando disponible para los consumidores.
- **Las unidades de enlace y los scripts de compilación se almacenan en caché.** Los ejecutables, pruebas, macros procedimentales y ejecuciones de scripts de compilación se identifican mediante claves y se almacenan, no solo las bibliotecas `rlib`.
- **Las compilaciones concurrentes cooperan.** Bloqueos por clave: dos terminales o worktrees que necesitan la misma dependencia la compilan una sola vez. Un único servidor de trabajos (jobserver) limita el paralelismo para todos los procesos de Mitts en la máquina.
- **Uso compartido seguro.** Los objetos del almacén son inmutables: publicados atómicamente, almacenados en modo de solo lectura y restaurados mediante reflink o copia. `MITTS_VERIFY_RESTORE=1` vuelve a calcular el hash de cada objeto antes de restaurarlo (el valor predeterminado confía en el almacén, lo que ahorra una lectura de cada byte restaurado). Donde el sistema de archivos no admite reflink, la restauración crea un enlace duro (hardlink) del objeto del almacén de solo lectura dentro de `target/` en lugar de copiar sus bytes: el inodo compartido permanece en `0o444` (`0o555` para ejecutables), el índice de target rechaza un enlace que haya adquirido bits de escritura y Mitts solo reemplaza los archivos del espacio de trabajo mediante renombrado. Nunca habrá un enlace con permisos de escritura a un objeto del almacén. `MITTS_STORE_HARDLINKS=0` mantiene copias privadas para espacios de trabajo donde otras herramientas escriben in situ.
- **Preparación del espacio de trabajo con zero-copy.** Los alias de bibliotecas dentro del espacio de trabajo, los binarios secundarios y la preparación atómica de unidades utilizan enlaces duros y promoción por renombrado dentro del espacio de trabajo sin copiar bytes. Cuando `MITTS_CACHE_DIR` y el `target/` del espacio de trabajo residen en el mismo volumen del sistema de archivos, las promociones de preparación operan sin copiar bytes; a través de puntos de montaje distintos, recurren de manera transparente a una copia segura.
- **Una ruta rápida sin operaciones (no-op fast path).** Un espacio de trabajo sin cambios omite la planificación y la generación de claves.
- **Una única CLI.** `build`, `run`, `check`, `clippy`, `fmt`, `test` (con doctests), `coverage`, `explain`, `fetch`, `gc`, `clean`.

<a id="quick-start"></a>
## Inicio rápido

Instala la versión nativa firmada más reciente:

```sh
curl -fsSL https://mittsbuild.com/install.sh | sh
```

Para compilar desde el código fuente del repositorio, primero instala la última versión publicada de Mitts y luego usa Mitts para compilar el repositorio:

```sh
git clone https://github.com/alex09x/mitts
cd mitts
mitts build --release
MITTS_JOBS=2 mitts build     # limit concurrent native build units
install -m755 target/release/mitts "$HOME/.cargo/bin/mitts"
```

Luego ejecútalo dentro de cualquier espacio de trabajo de Cargo:

```sh
mitts build                  # compile into target/debug/
mitts build --release
mitts run                    # build and run a binary
mitts test                   # unit, integration and doc tests
mitts test --tests -p foo    # Cargo-style target selection
mitts test -c                # every test in its own process, all the cores (-c=8: 8): docs/test-concurrency.md
mitts test --all-targets --no-run  # compile tests, examples and benchmarks
mitts bench                  # build benchmark targets (bench profile) and run them with --bench
mitts bench --bench speed -- --save-baseline main
# The cache stays under MITTS_CACHE_MAX_BYTES (default 100G) by itself: docs/cache-limits.md
mitts llvm-cov --ignore-filename-regex 'tests/' --fail-under-lines 80
mitts check | clippy | fmt | coverage
mitts explain                # the plan, units and why each is cached or not
mitts fetch                  # prefetch dependencies into the source store
mitts package                # create and verify a source archive
mitts publish --dry-run      # prepare publication without uploading
mitts gc                     # prune the shared store within quotas
mitts clean                  # remove this workspace's Mitts artifacts
```

Para proyectos de Git, el empaquetado requiere actualmente un repositorio limpio con el paquete en su raíz. Los patrones `include` del manifiesto pueden seleccionar archivos ignorados; los paquetes anidados del espacio de trabajo y los directorios con enlaces simbólicos aún no son compatibles. `mitts publish` tiene como destino crates.io y lee el token de `CARGO_REGISTRY_TOKEN`.

Mitts escribe las salidas de compilación en los directorios habituales de Cargo `target/debug/` y `target/release/`. `CARGO_TARGET_DIR` y `--target-dir` seleccionan una raíz de salida diferente. Su caché compartida es independiente (`--cache-dir`, si no `$MITTS_CACHE_DIR`, si no `$XDG_CACHE_HOME/mitts`, si no `~/.cache/mitts`). El almacén de artefactos, la caché de acciones, los bloqueos, el jobserver, el almacén de fuentes, los espejos de git y la caché de índices residen bajo esa única raíz.

Las dependencias de Git, públicas o privadas, son clonadas por el propio Mitts usando el `git` del sistema, por lo que tus claves SSH, agente, asistentes de credenciales y `GIT_SSH_COMMAND` se aplican exactamente igual que en la línea de comandos; las solicitudes interactivas están deshabilitadas, por lo que una credencial faltante falla de inmediato en lugar de bloquear el proceso. Nada pasa a través de Cargo. Una copia de trabajo que Cargo ya haya creado en `~/.cargo/git/checkouts` se reutiliza en lugar de clonarse nuevamente.

Cualquier otra forma en que Cargo obtiene un crate funciona de la misma manera, a partir de la misma configuración:

- **Registros alternativos**: `[registries.<name>] index = "sparse+https://…"` y `registry = "<name>"` en la dependencia. Los tokens provienen de los proveedores de credenciales de Cargo, en el orden de Cargo: `cargo:token` (el entorno y `credentials.toml`), `cargo:macos-keychain`, `cargo:libsecret`, `cargo:token-from-stdout <command>`, y complementos como `cargo-credential-1password` mediante el protocolo de proveedores de credenciales, configurado por `[registry] global-credential-providers`, `[registries.<name>] credential-provider` y `[credential-alias]`. Un registro cuyo `config.json` requiere autenticación (`auth-required`) falla con un mensaje claro cuando ningún proveedor tiene un token. Los registros basados en índice git se rechazan: utiliza el protocolo sparse.
- **Espejos (Mirrors)**: `[source.crates-io] replace-with = "<name>"` con `[source.<name>] registry = "sparse+https://…"`. Los bytes proceden del espejo, mientras que `Cargo.lock` sigue indicando crates.io, exactamente como lo genera Cargo.
- **Fuentes vendoreadas**: `[source.vendored-sources] directory = "vendor"` de `cargo vendor`. Los paquetes se verifican contra su `.cargo-checksum.json` y se copian al almacén; no se realiza ningún acceso a la red, incluidos los paquetes git. Sin un lockfile, el directorio vendor actúa como índice.
- **`[patch]`**: `[patch.crates-io] foo = { path = … }` o `{ git = … }` reemplaza el paquete del registro; sin un lockfile, el reemplazo se resuelve y fija, debiendo satisfacer el requisito de versión, igual que con Cargo.

La sección `[env]` en una configuración de Cargo se aplica a rustc, a los scripts de compilación y a los programas que inician `mitts run` y `mitts test`, siguiendo las reglas `force` y `relative` de Cargo; los archivos de configuración son entradas, por lo que un valor modificado recompila aquello que lo leyó.

Sin un `Cargo.lock`, Mitts resuelve uno de la misma manera que Cargo: la versión más alta no retirada (non-yanked) por requisito, las características fluyendo a lo largo de las aristas, las dependencias opcionales incluidas solo cuando una característica las activa, una versión por rango de compatibilidad. El resultado es un lockfile que Cargo acepta como propio.

<a id="how-it-works"></a>
## Cómo funciona

```
Cargo.toml + Cargo.lock
        │
        ▼
 resolve ─── reuse Cargo.lock, or resolve against the crates.io sparse index
        │
        ▼
 fetch ───── download .crate archives in parallel, verify, unpack once per machine;
        │    clone git dependencies with the system git at the locked commit
        │
        ▼
 plan ────── compilation units: lib · bin · test · build script · proc macro
        │    with features, targets and dependency edges (Cargo's resolver rules)
        ▼
 key ─────── SHA-256 over toolchain, target, profile, flags, features,
        │    source contents, dependency keys and tracked environment
        ▼
 execute ─── parallel DAG of direct rustc calls, pipelined on .rmeta
             hit  → restore from the store (reflink, read-only link or copy)
             miss → lock the key, compile, publish atomically to the store
```

La clave de una unidad incluye las **claves** de sus dependencias, no sus bytes de salida, por lo que cualquier cambio por debajo de una unidad modifica su clave. Los scripts de compilación se ejecutan en un directorio de preparación; sus directivas (`rustc-cfg`, `rustc-link-*`, `rustc-env`, …) se incorporan a la clave de la unidad que configuran, y sus declaraciones `rerun-if-*` deciden cuándo se puede reutilizar una ejecución almacenada en caché.

Tras una compilación exitosa, Mitts registra una instantánea (snapshot) del espacio de trabajo. Si nada ha cambiado en él, la siguiente compilación salta directamente al resultado. Cualquier discrepancia recurre a la compilación completa basada en claves, por lo que la ruta rápida nunca omite el trabajo prescrito por el protocolo.

El protocolo completo —derivación de claves, bloqueos, scripts de compilación, ruta rápida, diseño del almacén y recolección de basura— se encuentra en [docs/architecture.md](../docs/architecture.md).

<a id="architecture"></a>
## Arquitectura

Mitts consiste en un núcleo independiente del lenguaje con adaptadores de lenguaje sobre él. El núcleo desconoce los detalles de Cargo; el adaptador de Rust convierte un espacio de trabajo de Cargo en acciones identificadas por clave.

```
                 ┌──────────────────────────────────────────────┐
  mitts CLI ───▶ │ Rust adapter                                 │
  (src/cli)      │ manifest · resolver · fetcher · features     │
                 │ planner · keys · DAG runner · build scripts  │
                 ├──────────────────────────────────────────────┤
                 │ C/C++ adapter (experimental, Mitts.toml)     │
                 └──────────────────────┬───────────────────────┘
                                        │ actions + keys
                 ┌──────────────────────▼───────────────────────┐
                 │ Core                                         │
                 │ content-addressed store · action cache       │
                 │ key locks · jobserver · fast path · GC       │
                 └──────────────────────────────────────────────┘
```

| Ruta | Qué reside allí |
| --- | --- |
| `src/cli/` | Controladores de comandos: `build`, `test`, `clippy`, `explain`, … |
| `src/adapter/rust/` | Semántica de Cargo: manifiestos, lockfile, índice sparse, resolución de características, planificación de unidades, claves de acciones, ejecutor DAG, scripts de compilación, doctests |
| `src/adapter/c/` | Adaptador experimental C/C++ sobre el mismo almacén |
| `src/core/` | Almacén independiente del lenguaje (`cas.rs`), caché de acciones, bloqueos, jobserver, ruta rápida (`build_lock*.rs`), GC, reflinks |
| `docs/` | [Protocolo](../docs/architecture.md), [decisiones de diseño](../docs/decisions), [escenarios](../docs/scenarios) |
| `tests/` | Pruebas de integración, incluidas pruebas de paridad con Cargo y de invalidación de caché |
| `scripts/` | Conjunto de herramientas para pruebas comparativas (benchmarks) y auditorías |
| `fixtures/` | Proyectos mínimos en Rust y C utilizados por las pruebas |

<a id="what-invalidates-the-cache"></a>
## Qué invalida la caché

Un acierto de caché requiere que coincidan todas las entradas indicadas a continuación. Si Mitts no puede observar una entrada, no reutiliza el resultado.

| Entrada | Rastreado como |
| --- | --- |
| Cadena de herramientas | Salida de `rustc -vV` |
| Destino y perfil | Tripleta de destino, configuración de perfil, `RUSTFLAGS` y rustflags de la configuración de Cargo |
| Características | Conjunto resuelto de características de la unidad (resolver 1 o 2, como haría Cargo) |
| Fuentes | Hashes de contenido de los archivos del paquete y de cada archivo que rustc informe haber leído (`include_str!`, `include_bytes!`, módulos) |
| Dependencias | Las claves de acción de todas las unidades de dependencia |
| Entorno | Parámetros relevantes de `CARGO_*` y `RUST_*`, y variables leídas mediante `env!` / `option_env!`, registrando explícitamente `<unset>` |
| Configuración de Cargo | Los archivos `.cargo/config(.toml)` que leería Cargo |
| Scripts de compilación | Sus directivas, `OUT_DIR`, el contenido de las rutas `rerun-if-changed` y los valores de las variables `rerun-if-env-changed` |

Los programas `rustc-wrapper` y `rustc-workspace-wrapper` configurados se invocan para la compilación. Dado que un wrapper arbitrario puede depender de entradas no rastreadas, Mitts deshabilita la reutilización de la caché de acciones y de la caché de scripts de compilación mientras cualquiera de los dos wrappers esté activo. Esto puede hacer que esas compilaciones sean más lentas.

Algunas macros procedimentales leen archivos de tu paquete durante su expansión: `sqlx` comprueba consultas contra `.sqlx/`, `askama` compila `templates/`, `rust-embed` e `include_dir` incrustan directorios completos. rustc solo tiene conocimiento de estas lecturas cuando la macro las registra, por lo que debes declarar estos crates en el `Cargo.toml` raíz:

```toml
[workspace.metadata.mitts]          # [package.metadata.mitts] in a single crate
impure-proc-macros = ["sqlx", "askama", "rust-embed"]
```

Los crates que dependen de una macro listada se identifican por cada archivo en su paquete y por su ruta absoluta: una plantilla editada los recompila, y otra copia de trabajo nunca los reutiliza. Cualquier otra macro se considera pura, y las unidades que dependen de ella se comparten entre worktrees. Nombra el crate del que dependes; el crate de macros que este incluye (`sqlx-macros`) también funciona.

<a id="comparison"></a>
## Comparación

Una comparación cualitativa de cómo cada herramienta reutiliza el trabajo. No constituye una afirmación de rendimiento.

| | Cargo | Cargo + sccache | Mitts |
| --- | --- | --- | --- |
| Lee `Cargo.toml` / `Cargo.lock` tal cual | sí | sí | sí |
| Reutilización entre espacios de trabajo y worktrees | no (por cada `target/`) | sí, por llamada a `rustc` | sí, por unidad |
| La clave depende de la ruta de la copia de trabajo | — | sí, a menos que esté normalizada | no para crates del registro; para crates del espacio de trabajo solo si no son elegibles |
| Almacena en caché unidades `bin`, pruebas y `proc-macro` | — | no | sí |
| Almacena en caché ejecuciones de scripts de compilación | no | no | sí |
| Compilaciones concurrentes de una misma dependencia | esperan en el bloqueo de `target/` | ambas pueden compilar si hay fallo de caché | una compila, la otra reutiliza |
| Caché remota / de equipo | no | sí (S3, GCS, Redis, …) | planificado |
| Windows | sí | sí | no |

Bazel y Buck2 resuelven un problema más amplio —compilaciones herméticas y multilingües con ejecución remota— pero requieren sus propios archivos de compilación en lugar de `Cargo.toml`.

<a id="limitations"></a>
## Limitaciones

Mitts se encuentra en fase pre-release. Ten en cuenta lo siguiente antes de confiar en él:

- **Solo Linux y macOS.** Los bloqueos y el jobserver son compatibles con POSIX.
- **Solo destino anfitrión (Host target).** Aún no se admite la compilación cruzada.
- **Sin caché remota todavía.** El almacén es local por máquina.
- **Solo registros sparse.** Se rechazan con sugerencias los registros servidos mediante índice git (`registry+…` distinto de crates.io, o `[source] registry = "https://…"` sin `sparse+`) y las fuentes `local-registry`.
- **Las credenciales solo se leen, nunca se escriben.** `cargo login` sigue siendo responsabilidad de Cargo; `cargo:wincred` es exclusivo de Windows y se omite.
- **Algunas unidades nunca se almacenan en caché:** crates de tipo `cdylib`, `dylib` o `staticlib`, y crates cuyas lecturas de `env!` no se pueden rastrear. Siempre se compilan; son informados por `mitts explain`.
- **Crates del espacio de trabajo entre copias de trabajo.** Tus propios crates se reutilizan entre worktrees únicamente cuando su clave puede independizarse de la ruta (sin lecturas de `CARGO_MANIFEST_DIR`, sin script de compilación, sin macros procedimentales impuras declaradas). De lo contrario, la clave incluye la ruta absoluta. En macOS, las entradas debug stabs del enlazador se escriben relativas al espacio de trabajo (`-oso_prefix`), por lo que los binarios enlazados también se comparten; inicia el depurador en la raíz del espacio de trabajo para que localice los archivos objeto.
- **Las macros que leen tus archivos deben ser declaradas por ti.** Una macro procedimental se considera pura a menos que el proyecto la enumere (ver arriba). Los dependientes de una macro listada rastrean cada archivo de su paquete; una macro no listada que lea un archivo sin registrarlo ante rustc puede generar un resultado desactualizado, exactamente igual que con Cargo.
- **Los scripts de compilación no están en un sandbox.** Un script que lea archivos o variables no declarados mediante `rerun-if-*` puede producir un resultado obsoleto, exactamente igual que con Cargo.
- **Las características no compatibles de Cargo fallan de forma explícita.** Por ejemplo, `--profile` o patrones globales con comodines en directorios intermedios en `workspace.members`. Mitts informa de ellos en lugar de hacer conjeturas.
- **Zero-copy en el mismo sistema de archivos.** La promoción de unidades sin copia desde la preparación requiere que `MITTS_CACHE_DIR` y el directorio `target/` del espacio de trabajo residan en el mismo punto de montaje del sistema de archivos. Al cruzar límites de sistemas de archivos, se recurre de forma segura a la copia estándar.

**No uses Mitts todavía** si necesitas Windows, compilación cruzada, una caché de equipo o una versión estable. En esos casos, Cargo (opcionalmente con sccache) es la opción segura.

<a id="performance"></a>
## Rendimiento

Medido con Mitts `0.3.17` en **100 proyectos de Rust de código abierto** en macOS (Apple M2 Ultra, APFS) y Linux ARM64 (XFS), y **90** en Linux x86_64 (AMD Ryzen 9 9950X, XFS), frente a `cargo` estándar y `cargo` con `sccache` (`0.17.0`). Cada lectura corresponde a la mediana de 3 repeticiones verificando la paridad en diagnósticos, códigos de salida y resultados de pruebas; todos los proyectos medidos permanecen en los totales (`0`, `7` y `0` ejecuciones marcadas con ruido), y los factores de aceleración se truncan en lugar de redondearse al alza ([ROADMAP M8](../ROADMAP.md)).

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="../docs/img/bench-scenarios-dark.svg">
    <source media="(prefers-color-scheme: light)" srcset="../docs/img/bench-scenarios-light.svg">
    <img src="../docs/img/bench-scenarios-light.svg" width="920" alt="Tiempo medio de compilación por proyecto en seis escenarios: Cargo, Cargo + sccache y Mitts">
  </picture>
</p>

La longitud de las barras muestra la media aritmética de las medianas por proyecto (proporcional al tiempo real total del corpus), con las medias geométricas por proyecto junto a cada fila. En compilaciones con múltiples worktrees, cambio de rama y reconstrucción sin cambios (no-op por defecto), Mitts reduce el tiempo medio en **5.8–10.8×** en macOS. Al **editar un archivo y recompilar** (`n = 71` en macOS y Linux ARM64, `64` en Linux x86_64), Mitts es más rápido en media geométrica (**1.1×** en macOS y Linux ARM64, **1×** en Linux x86_64; más rápido en `49/71`, `42/71` y `43/64` proyectos), mientras que su media aritmética es más lenta (**0.7–0.8×**) porque algunos binarios grandes de un solo crate aprovechan la compilación incremental intracrate de `rustc` bajo `cargo`, mientras que Mitts almacena en caché unidades de crate completas. Una **compilación en frío aislada** con caché vacía se sitúa en **0.8–1×** frente a `cargo` estándar y es **1.4×** más rápida en media geométrica que `cargo` con `sccache` en frío.

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="../docs/img/bench-swarm-dark.svg">
    <source media="(prefers-color-scheme: light)" srcset="../docs/img/bench-swarm-light.svg">
    <img src="../docs/img/bench-swarm-light.svg" width="920" alt="Marcador de compilación de 12 worktrees en 100 proyectos de Rust: tiempo, CPU, ejecuciones del compilador, escritura en disco y RSS pico">
  </picture>
</p>

Compilar **12 worktrees en paralelo** sobre 4 commits en los 100 proyectos de macOS toma **76 min** con `cargo`, **74 min** con `cargo` + `sccache` y **13 min** con Mitts (**5.8×** menos tiempo real; **7×** en Linux x86_64, **3.6×** en Linux ARM64), reduciendo las invocaciones de `rustc` de **90,961** a **6,905** (**13.1×**) y las escrituras en disco de **641 GB** a **111 GB** (**5.7×**). Sumado en los 12 worktrees, el pico de RSS de procesos hijos (`ru_maxrss`) promedia **2.4–3.4 GB** por proyecto con Mitts frente a **6.1–6.5 GB** con `cargo` estándar (**1.8–2.5×** menos), ya que los worktrees en caché no lanzan `rustc`; dentro de un único worktree, el proceso hijo más pesado promedia **451–520 MB** con Mitts frente a **539–582 MB** con `cargo` estándar (`cargo` + `sccache` registra **443–483 MB** en `wait4` porque el servidor desacoplado de `sccache` se ejecuta fuera del árbol de procesos medido).

<a id="status"></a>
## Estado

| Área | Estado |
| --- | --- |
| Lectura de `Cargo.toml` / `Cargo.lock`, resolución mediante índice sparse | completado |
| Resolución de características (resolver 1 y 2) | completado |
| Descargador paralelo y almacén global de fuentes | completado |
| Ejecutor DAG nativo, `rustc` en cascada (pipelined), jobserver | completado |
| Almacén direccionado por contenido y caché de acciones | completado |
| Ejecución y almacenamiento en caché de scripts de compilación | completado |
| Ruta rápida sin operaciones (No-op fast path) | completado |
| `check`, `clippy`, `fmt`, `test` (incl. doctests), `coverage` | completado |
| Recolección de basura conservadora con cuotas | completado |
| Adaptador C/C++ sobre el mismo almacén | experimental |
| Claves independientes de la ruta para crates del espacio de trabajo | parcial |
| Estudio de benchmarks reproducible | completado |
| Caché remota / para equipos | planificado |
| Windows, compilación cruzada | no compatible |

Hitos y evidencias: [ROADMAP.md](../ROADMAP.md).

<a id="faq"></a>
## Preguntas frecuentes (FAQ)

**¿Reemplaza Mitts a Cargo?**
Mitts lee los manifiestos y archivos de bloqueo existentes de Cargo y proporciona el flujo de trabajo de compilación y pruebas de Rust para este proyecto. Sus propios flujos de trabajo de CI y de publicación utilizan Mitts sin invocar a Cargo. Cargo sigue estando disponible para los usuarios externos que lo necesiten.

**¿Puedo usarlo en CI?**
Sí. El CI de este repositorio instala la versión estable más reciente de Mitts y luego la usa para las compilaciones nativas, pruebas, cobertura, formateo y Clippy. El almacén de artefactos compartido es local para cada ejecutor; una caché remota entre máquinas aún no está disponible.

**¿Entrará en conflicto con rust-analyzer o las salidas existentes de Cargo?**
Mitts utiliza las mismas rutas de salida predeterminadas que Cargo. Su almacén de artefactos compartido permanece independiente, y `mitts clean` elimina únicamente las salidas indexadas de Mitts que todavía coinciden con su contenido registrado; preserva los archivos no relacionados o reemplazados de Cargo. Si Cargo mantiene el bloqueo del directorio de compilación de un perfil, la limpieza se omite hasta que esa compilación termine.

**¿Dónde está la caché y cómo la limpio?**
`--cache-dir`, si no `$MITTS_CACHE_DIR`, `$XDG_CACHE_HOME/mitts` o `~/.cache/mitts`; todo lo que Mitts almacena en caché reside bajo esa única raíz. Utiliza `mitts gc` para limpiar dentro de las cuotas; eliminar el directorio también es seguro cuando no hay ninguna compilación en ejecución.

**¿Por qué no se almacenó en caché una unidad?**
Ejecuta `mitts explain`. Muestra cada unidad y la razón por la cual es o no apta para la caché.

<a id="design-rules"></a>
## Reglas de diseño

- Mantener la orquestación de compilación y caché independiente del lenguaje. La semántica de los lenguajes reside en adaptadores; Rust es el primero.
- Un acierto de caché requiere entradas completas y rastreadas. Las entradas desconocidas se tratan como modificadas.
- Los artefactos compartidos son inmutables y se verifican antes de su uso. El estado mutable permanece en cada espacio de trabajo. Sin enlaces duros a archivos con permisos de escritura, sin trucos de marcas de tiempo.
- La limpieza nunca toca los artefactos de una compilación activa.

Reglas para colaboradores y agentes: [AGENTS.md](../AGENTS.md).
Decisiones de arquitectura: [docs/decisions](../docs/decisions).

<a id="author"></a>
## Autor

[Alexander Panasenko](https://prod.codes/about/) · alex@prod.codes ·
[mittsbuild.com](https://mittsbuild.com)

<a id="citation"></a>
## Cómo citar

Para citar Mitts, usa la versión archivada. El DOI de concepto [10.5281/zenodo.24030067](https://doi.org/10.5281/zenodo.24030067) siempre apunta a la última versión; el DOI de la versión 0.3.29 es [10.5281/zenodo.24030068](https://doi.org/10.5281/zenodo.24030068). La opción «Cite this repository» de GitHub lee [CITATION.cff](../CITATION.cff).

```bibtex
@software{panasenko_2026_24030068,
  author    = {Panasenko, Alexander},
  title     = {Mitts Build — Rust-first build system with a machine-wide, content-addressed artifact cache},
  year      = {2026},
  publisher = {Zenodo},
  version   = {0.3.29},
  doi       = {10.5281/zenodo.24030068},
  url       = {https://doi.org/10.5281/zenodo.24030068}
}
```

<a id="license"></a>
## Licencia

Doble licencia bajo [MIT](../LICENSE-MIT) O [Apache-2.0](../LICENSE-APACHE), a tu elección.
