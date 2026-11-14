<!-- translation-source: 1a926080b49fae712d44fdae44fb6f96b80d3499 -->

> [!NOTE]
> Dies ist eine Übersetzung der offiziellen englischsprachigen Dokumentation für Commit [1a92608](https://github.com/alex09x/mitts/blob/1a926080b49fae712d44fdae44fb6f96b80d3499/README.md).
> Die kanonische und aktuellste Version ist die [englische README](https://github.com/alex09x/mitts/blob/main/README.md).

<p align="center">
  <img src="../site/logo.svg" width="96" height="96" alt="Mitts Build logo">
</p>

<h1 align="center">Mitts Build</h1>

<p align="center">
  <b>Build-Arbeit, lückenlos erfasst.</b><br>
  Ein Rust-orientiertes Build-System mit einem maschinenweiten, inhaltsadressierten Artefakt-Cache
  und expliziter Eingabeerfassung. Es liest Ihre vorhandenen Dateien <code>Cargo.toml</code> und <code>Cargo.lock</code>.
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
  <a href="#quick-start">Schnellstart</a> ·
  <a href="#how-it-works">Funktionsweise</a> ·
  <a href="#architecture">Architektur</a> ·
  <a href="../docs/architecture.md">Protokoll</a> ·
  <a href="#limitations">Einschränkungen</a> ·
  <a href="../ROADMAP.md">Roadmap</a>
</p>

<p align="center">
  <a href="https://github.com/alex09x/mitts/blob/main/README.md">English</a> ·
  <a href="https://github.com/alex09x/mitts/blob/main/readme/README.ru.md">Русский</a> ·
  <a href="https://github.com/alex09x/mitts/blob/main/readme/README.zh-CN.md">简体中文</a> ·
  <a href="https://github.com/alex09x/mitts/blob/main/readme/README.es.md">Español</a> ·
  <a href="https://github.com/alex09x/mitts/blob/main/readme/README.pt-BR.md">Português (Brasil)</a> ·
  <a href="https://github.com/alex09x/mitts/blob/main/readme/README.ja.md">日本語</a> ·
  <b><a href="https://github.com/alex09x/mitts/blob/main/readme/README.de.md">Deutsch</a></b> ·
  <a href="https://github.com/alex09x/mitts/blob/main/readme/README.fr.md">Français</a> ·
  <a href="https://github.com/alex09x/mitts/blob/main/readme/README.ko.md">한국어</a> ·
  <a href="https://github.com/alex09x/mitts/blob/main/readme/README.ar.md">العربية</a>
</p>

---

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="../docs/img/bench-hero-dark.svg">
    <source media="(prefers-color-scheme: light)" srcset="../docs/img/bench-hero-light.svg">
    <img src="../docs/img/bench-hero-light.svg" width="920" alt="Das Bauen von 12 Worktrees von 100 Rust-Projekten auf einem Mac dauert 13 Minuten mit Mitts und 76 Minuten mit Cargo; die Compiler-Läufe sinken von 90,961 auf 6,905 und die geschriebene Datenmenge von 641 GB auf 112 GB">
  </picture>
</p>

<p align="center">
  <sub>100 Open-Source-Projekte (90 unter Linux x86_64), jeweils 12 Worktrees gleichzeitig gebaut, auf drei Maschinen. Cargo und Cargo + sccache als Vergleichsbasis, einschließlich der Fälle, in denen Mitts nicht schneller ist: <a href="#performance">Leistung</a> · <a href="https://mittsbuild.com/the-hundred/">The Hundred</a></sub>
</p>

Mitts plant den Workspace eigenständig und ruft `rustc` direkt auf. Jede
Kompilieraktion erhält einen Schlüssel, der aus all ihren Eingaben abgeleitet wird. Die Ausgabe wird
einmalig in einem maschinenweiten, unveränderlichen Speicher abgelegt und in jeden Workspace
wiederhergestellt, der sie benötigt – über Projekte, Worktrees und Branches hinweg.

Kann eine Eingabe nicht lückenlos erfasst werden, wird die Aktion neu gebaut statt erraten.

## Das Problem

Der Build-Cache von Cargo liegt im Verzeichnis `target/` des jeweiligen Workspace. Alles,
was ein neues `target/` erzeugt, beginnt bei null:

- ein zweiter Checkout oder ein `git worktree` desselben Repositories,
- das Wechseln zu einem Branch, der `Cargo.lock` ändert, und das anschließende Zurückwechseln,
- ein weiteres Projekt, das von denselben Crates mit denselben Features abhängt,
- parallele Coding-Agenten, von denen jeder in seinem eigenen Worktree arbeitet.

Dasselbe `serde`, `syn` oder `tokio` wird immer wieder aufs Neue kompiliert, und jede Kopie
belegt Speicherplatz auf der Festplatte.

Die üblichen Ausweichlösungen haben ihre Tücken:

- **Ein gemeinsam genutztes `CARGO_TARGET_DIR`** serialisiert Builds hinter einer einzigen Sperre, und
  Projekte mit unterschiedlichen Feature-Sets überschreiben gegenseitig ihre Artefakte.
- **Ein Compiler-Wrapper wie sccache** sieht jeweils nur einen einzelnen `rustc`-Aufruf zur Zeit. Sein
  Schlüssel hängt von absoluten Pfaden ab, sofern diese nicht normalisiert sind, und Crates,
  die den Linker aufrufen (`bin`, `proc-macro`, `dylib`), werden nicht zwischengespeichert. Cargo plant
  weiterhin den Build und führt jedes Build-Skript aus.

Mitts wählt einen anderen Weg: Es ersetzt den Build-Executor von Cargo, nicht den Compiler.
Es sieht den gesamten Einheitengraphen, sodass es jede Einheit anhand der Schlüssel ihrer Abhängigkeiten
schlüsseln, Link-Einheiten und Build-Skript-Läufe zwischenspeichern und mehrere Builds sicher einen
gemeinsamen Speicher nutzen lassen kann.

## Highlights

- **Wiederverwendung über Workspaces hinweg.** Ein einziger Speicher pro Maschine. Eine für
  einen Checkout kompilierte Abhängigkeit wird bei identischen Eingaben im nächsten wiederhergestellt.
- **Vollständige Schlüssel.** Toolchain, Target, Profil, Flags, Features, Quellcode-Inhalte,
  Abhängigkeitsschlüssel und erfasste Umgebung. Unbekannte Eingaben bedeuten einen
  Neubau.
- **Cargo-Manifeste bleiben der Vertrag.** Kein neues Manifest, keine zweite Lock-Datei.
  Die Build-, Test-, Lint-, Coverage- und Release-Quellcode-Workflows dieses Repositories laufen
  über Mitts; sie rufen die Cargo-CLI nicht auf. Externe Cargo-Interoperabilität
  bleibt für Anwender weiterhin verfügbar.
- **Link-Einheiten und Build-Skripte werden zwischengespeichert.** Binärdateien, Tests, Proc-Makros
  und Build-Skript-Ausführungen werden geschlüsselt und gespeichert – nicht nur `rlib`s.
- **Gleichzeitige Builds kooperieren.** Schlüsselbasierte Sperren: Zwei Terminals oder Worktrees,
  die dieselbe Abhängigkeit benötigen, kompilieren sie nur einmal. Ein einziger Jobserver begrenzt die
  Parallelität für alle Mitts-Prozesse auf der Maschine.
- **Sichere gemeinsame Nutzung.** Speicherobjekte sind unveränderlich: Sie werden atomar veröffentlicht,
  schreibgeschützt gespeichert und via Reflink oder Kopie wiederhergestellt. `MITTS_VERIFY_RESTORE=1` berechnet
  den Hash jedes Objekts vor der Wiederherstellung neu (standardmäßig wird dem Speicher vertraut, was einen Lesevorgang
  jedes wiederhergestellten Bytes einspart). Unterstützt das Dateisystem kein Reflink, verlinkt eine Wiederherstellung das
  schreibgeschützte Speicherobjekt per Hardlink in `target/`, anstatt seine Bytes zu kopieren: Der geteilte Inode
  behält `0o444` (`0o555` für ausführbare Dateien), der Target-Index weist Links ab, die
  Schreibrechte erhalten haben, und Mitts ersetzt Workspace-Dateien ausschließlich durch Umbenennen. Niemals ein beschreibbarer
  Link zu einem Speicherobjekt. `MITTS_STORE_HARDLINKS=0` behält private Kopien für Workspaces bei,
  in die andere Werkzeuge direkt an Ort und Stelle schreiben.
- **Zero-Copy-Workspace-Staging.** Workspace-interne Bibliotheks-Aliase, sekundäre Binärdateien
  und atomares Einheiten-Staging nutzen interne Hardlinks und Rename-Promotion ohne
  Kopieren von Bytes. Wenn `MITTS_CACHE_DIR` und das `target/` des Workspace auf demselben
  Dateisystem-Volume liegen, arbeiten Staging-Promotions ohne Kopieren von Bytes; über separate
  Mounts hinweg greifen sie transparent auf sicheres Kopieren zurück.
- **Ein schneller Pfad bei Nichtstun (No-Op).** Ein unveränderter Workspace überspringt Planung und Schlüsselgenerierung.
- **Eine einzige CLI.** `build`, `run`, `check`, `clippy`, `fmt`, `test` (mit Doctests),
  `coverage`, `explain`, `fetch`, `gc`, `clean`.

<a id="quick-start"></a>
## Schnellstart

Installieren Sie das neueste signierte native Release:

```sh
curl -fsSL https://mittsbuild.com/install.sh | sh
```

Um den Quellcode-Checkout zu bauen, installieren Sie zuerst das neueste veröffentlichte Mitts und verwenden
Sie dann Mitts, um den Checkout zu bauen:

```sh
git clone https://github.com/alex09x/mitts
cd mitts
mitts build --release
MITTS_JOBS=2 mitts build     # limit concurrent native build units
install -m755 target/release/mitts "$HOME/.cargo/bin/mitts"
```

Führen Sie es anschließend in einem beliebigen Cargo-Workspace aus:

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

Für Git-Projekte erfordert das Paketieren derzeit ein sauberes Repository mit dem Paket im Stammverzeichnis.
`include`-Muster im Manifest können ignorierte Dateien auswählen; verschachtelte Workspace-Pakete
und Verzeichnisse mit symbolischen Links werden noch nicht unterstützt. `mitts publish` zielt auf crates.io ab und liest
das Token aus `CARGO_REGISTRY_TOKEN`.

Mitts schreibt Build-Ausgaben in die von Cargo gewohnten Verzeichnisse `target/debug/` und `target/release/`.
`CARGO_TARGET_DIR` und `--target-dir` wählen ein anderes Ausgabeverzeichnis.
Der geteilte Cache liegt separat
(`--cache-dir`, sonst `$MITTS_CACHE_DIR`, sonst `$XDG_CACHE_HOME/mitts`, sonst
`~/.cache/mitts`). Der Artefakt-Speicher, Action-Cache, Sperren, Jobserver, Quellcode-Speicher,
Git-Spiegelungen und Index-Cache liegen alle unter diesem einen Stammverzeichnis.

Git-Abhängigkeiten, ob öffentlich oder privat, werden von Mitts selbst mit dem System-`git`
geklont, sodass Ihre SSH-Schlüssel, Ihr Agent, Credential-Helper und `GIT_SSH_COMMAND` genau wie
auf der Befehlszeile greifen; interaktive Eingabeaufforderungen sind deaktiviert, sodass fehlende Zugangsdaten
zu einem direkten Fehler führen, anstatt zu blockieren. Nichts läuft über Cargo. Ein bereits von Cargo
unter `~/.cargo/git/checkouts` erstellter Checkout wird wiederverwendet, anstatt erneut geklont zu werden.

Jeder andere Weg, über den Cargo ein Crate bezieht, funktioniert auf dieselbe Weise aus derselben Konfiguration:

- **Alternative Registries**: `[registries.<name>] index = "sparse+https://…"`
  und `registry = "<name>"` an der Abhängigkeit. Tokens stammen von Cargos
  Credential-Providern in Cargos Reihenfolge: `cargo:token` (die Umgebung und
  `credentials.toml`), `cargo:macos-keychain`, `cargo:libsecret`,
  `cargo:token-from-stdout <command>` und Plugins wie
  `cargo-credential-1password` über das Credential-Provider-Protokoll,
  konfiguriert über `[registry] global-credential-providers`,
  `[registries.<name>] credential-provider` und `[credential-alias]`. Eine Registry,
  deren `config.json` `auth-required` angibt, schlägt mit einer verständlichen Meldung fehl, wenn kein
  Provider über ein Token verfügt. Git-Index-Registries werden abgelehnt: Verwenden Sie das Sparse-Protokoll.
- **Spiegelserver (Mirrors)**: `[source.crates-io] replace-with = "<name>"` mit
  `[source.<name>] registry = "sparse+https://…"`. Bytes stammen vom Spiegelserver,
  `Cargo.lock` benennt weiterhin crates.io, genau so, wie Cargo es schreibt.
- **Vendored-Quellen**: `[source.vendored-sources] directory = "vendor"` aus
  `cargo vendor`. Pakete werden gegen ihre `.cargo-checksum.json` geprüft
  und in den Speicher kopiert; nichts kontaktiert das Netzwerk, einschließlich
  Git-Paketen. Ohne Lock-Datei dient das Vendor-Verzeichnis als Index.
- **`[patch]`**: `[patch.crates-io] foo = { path = … }` oder `{ git = … }`
  ersetzt das Registry-Paket; ohne Lock-Datei wird der Ersatz
  aufgelöst und festgeschrieben und muss die Anforderung erfüllen, genau wie bei Cargo.

`[env]` in einer Cargo-Konfiguration wird auf rustc, Build-Skripte und die Programme angewendet,
die `mitts run` und `mitts test` starten, nach Cargos `force`- und `relative`-Regeln;
die Konfigurationsdateien sind Eingaben, sodass ein geänderter Wert alles neu baut, was ihn gelesen hat.

Ohne `Cargo.lock` löst Mitts eine Lock-Datei so auf, wie Cargo es tun würde: höchste
nicht zurückgezogene Version pro Anforderung, Features fließen entlang der Kanten, eine
optionale Abhängigkeit wird nur einbezogen, wenn ein Feature sie aktiviert, eine Version pro
Kompatibilitätsbereich. Das Ergebnis ist eine Lock-Datei, die Cargo als seine eigene akzeptiert.

<a id="how-it-works"></a>
## Funktionsweise

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

Der Schlüssel einer Einheit umfasst die **Schlüssel** ihrer Abhängigkeiten, nicht deren Ausgabe-Bytes;
eine Änderung an beliebiger Stelle unterhalb einer Einheit ändert somit deren Schlüssel. Build-Skripte laufen in einem
Staging-Verzeichnis; ihre Direktiven (`rustc-cfg`, `rustc-link-*`, `rustc-env`, …) fließen in
den Schlüssel der von ihnen konfigurierten Einheit ein, und ihre `rerun-if-*`-Deklarationen entscheiden,
wann ein zwischengespeicherter Lauf wiederverwendet werden kann.

Nach einem erfolgreichen Build zeichnet Mitts einen Schnappschuss des Workspace auf. Wenn sich darin
nichts geändert hat, springt der nächste Build direkt zum Ergebnis. Jede Abweichung greift auf
den vollständigen geschlüsselten Build zurück, sodass der schnelle Pfad niemals Arbeit überspringt,
die das Protokoll ausführen würde.

Das vollständige Protokoll – Schlüsselableitung, Sperren, Build-Skripte, der schnelle Pfad,
Speicherlayout und Garbage Collection – befindet sich in
[docs/architecture.md](../docs/architecture.md).

<a id="architecture"></a>
## Architektur

Mitts besteht aus einem sprachneutralen Kern mit darauf aufsetzenden Sprachadaptern. Der Kern weiß
nichts über Cargo; der Rust-Adapter überführt einen Cargo-Workspace in geschlüsselte
Aktionen.

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

| Pfad | Was dort liegt |
| --- | --- |
| `src/cli/` | Befehlshandler: `build`, `test`, `clippy`, `explain`, … |
| `src/adapter/rust/` | Cargo-Semantik: Manifeste, Lock-Datei, Sparse-Index, Feature-Auflösung, Einheitenplanung, Aktionsschlüssel, der DAG-Runner, Build-Skripte, Doctests |
| `src/adapter/c/` | Experimenteller C/C++-Adapter auf demselben Speicher |
| `src/core/` | Sprachneutraler Speicher (`cas.rs`), Action-Cache, Sperren, Jobserver, schneller Pfad (`build_lock*.rs`), GC, Reflinks |
| `docs/` | [Protokoll](../docs/architecture.md), [Designentscheidungen](../docs/decisions), [Szenarien](../docs/scenarios) |
| `tests/` | Integrationstests, einschließlich Cargo-Paritäts- und Cache-Invalidierungstests |
| `scripts/` | Benchmark- und Audit-Harness |
| `fixtures/` | Minimale, von Tests verwendete Rust- und C-Projekte |

## Was den Cache invalidiert

Ein Cache-Treffer erfordert die Übereinstimmung jeder einzelnen nachfolgenden Eingabe. Kann Mitts eine
Eingabe nicht erfassen, wird das Ergebnis nicht wiederverwendet.

| Eingabe | Erfasst als |
| --- | --- |
| Toolchain | Ausgabe von `rustc -vV` |
| Target und Profil | Target-Triple, Profileinstellungen, `RUSTFLAGS` und rustflags aus der Cargo-Konfiguration |
| Features | Das aufgelöste Feature-Set der Einheit (Resolver 1 oder 2, wie bei Cargo) |
| Quellcodedateien | Inhaltshashes der Paketdateien und jeder Datei, deren Einlesen rustc meldet (`include_str!`, `include_bytes!`, Module) |
| Abhängigkeiten | Die Aktionsschlüssel aller Abhängigkeitseinheiten |
| Umgebung | Relevante `CARGO_*`- und `RUST_*`-Einstellungen sowie über `env!` / `option_env!` gelesene Variablen, mit `<unset>` explizit erfasst |
| Cargo-Konfiguration | Die `.cargo/config(.toml)`-Dateien, die Cargo einlesen würde |
| Build-Skripte | Deren Direktiven, `OUT_DIR` und der Inhalt von `rerun-if-changed`-Pfaden sowie Werte von `rerun-if-env-changed`-Variablen |

Konfigurierte `rustc-wrapper`- und `rustc-workspace-wrapper`-Programme werden für
die Kompilierung aufgerufen. Da ein beliebiger Wrapper von nicht erfassten Eingaben abhängen kann, deaktiviert
Mitts die Wiederverwendung des Action-Cache und des Build-Skript-Cache, solange einer der Wrapper aktiv ist.
Dies kann diese Builds verlangsamen.

Manche prozeduralen Makros lesen während ihrer Expansion Dateien aus Ihrem Paket: `sqlx` prüft
Abfragen gegen `.sqlx/`, `askama` kompiliert `templates/`, `rust-embed` und
`include_dir` binden ganze Verzeichnisse ein. rustc erfährt von solchen Lesevorgängen nur dann,
wenn das Makro sie registriert. Deklarieren Sie diese Crates daher in der Root-Datei `Cargo.toml`:

```toml
[workspace.metadata.mitts]          # [package.metadata.mitts] in a single crate
impure-proc-macros = ["sqlx", "askama", "rust-embed"]
```

Crates, die von einem gelisteten Makro abhängen, werden über jede Datei in ihrem Paket
und über ihren absoluten Pfad geschlüsselt: Ein bearbeitetes Template baut sie neu, und ein
anderer Checkout verwendet sie niemals wieder. Jedes andere Makro wird als rein behandelt, und seine
abhängigen Einheiten werden über Worktrees hinweg geteilt. Benennen Sie das Crate, von dem Sie abhängen; das
Makro-Crate, das es einbindet (`sqlx-macros`), funktioniert ebenfalls.

## Vergleich

Ein qualitativer Vergleich der Wiederverwendung von Arbeitsergebnissen. Es handelt sich um
keine Leistungsbehauptung.

| | Cargo | Cargo + sccache | Mitts |
| --- | --- | --- | --- |
| Liest `Cargo.toml` / `Cargo.lock` wie gewohnt | ja | ja | ja |
| Wiederverwendung über Workspaces und Worktrees hinweg | nein (pro `target/`) | ja, pro `rustc`-Aufruf | ja, pro Einheit |
| Schlüssel hängt vom Checkout-Pfad ab | — | ja, sofern nicht normalisiert | nein für Registry-Crates; für Workspace-Crates nur dann, wenn dafür nicht geeignet |
| Cacht `bin`-, Test- und `proc-macro`-Einheiten | — | nein | ja |
| Cacht Build-Skript-Läufe | nein | nein | ja |
| Parallele Builds derselben Abhängigkeit | warten auf die `target/`-Sperre | beide kompilieren eventuell bei einem Miss | einer kompiliert, der andere verwendet wieder |
| Remote- / Team-Cache | nein | ja (S3, GCS, Redis, …) | geplant |
| Windows | ja | ja | nein |

Bazel und Buck2 lösen ein größeres Problem – hermetische, mehrsprachige Builds mit
Remote-Ausführung –, erfordern jedoch eigene Build-Dateien anstelle von `Cargo.toml`.

<a id="limitations"></a>
## Einschränkungen

Mitts befindet sich im Pre-Release-Stadium. Beachten Sie Folgendes, bevor Sie darauf aufbauen:

- **Nur Linux und macOS.** Sperren und der Jobserver basieren auf POSIX.
- **Nur Host-Target.** Cross-Kompilierung wird noch nicht unterstützt.
- **Noch kein Remote-Cache.** Der Speicher ist maschinenlokal.
- **Nur Sparse-Registries.** Eine Registry, die als Git-Index bereitgestellt wird (`registry+…`
  außerhalb von crates.io oder `[source] registry = "https://…"` ohne
  `sparse+`), sowie `local-registry`-Quellen werden mit Hinweisen abgelehnt.
- **Zugangsdaten werden nur gelesen, nie geschrieben.** `cargo login` bleibt Aufgabe von Cargo;
  `cargo:wincred` ist Windows-spezifisch und wird übersprungen.
- **Manche Einheiten werden niemals zwischengespeichert:** Crates vom Typ `cdylib`, `dylib` oder
  `staticlib` sowie Crates, deren `env!`-Lesevorgänge nicht erfasst werden können. Sie werden stets
  neu gebaut und von `mitts explain` ausgewiesen.
- **Workspace-Crates über Checkouts hinweg.** Eigene Crates werden über
  Worktrees hinweg nur dann wiederverwendet, wenn ihr Schlüssel pfadunabhängig gestaltet werden kann (keine
  `CARGO_MANIFEST_DIR`-Lesevorgänge, kein Build-Skript, kein deklariertes unreines Proc-Makro).
  Andernfalls enthält der Schlüssel den absoluten Pfad. Unter macOS werden die Debug-Stabs
  des Linkers relativ zum Workspace geschrieben (`-oso_prefix`), sodass auch gelinkte
  Binärdateien geteilt werden; starten Sie den Debugger im Workspace-Root, damit er die
  Objektdateien findet.
- **Dateien lesende Makros müssen deklariert werden.** Ein prozedurales Makro gilt als rein,
  sofern das Projekt es nicht deklariert (siehe oben). Abhängige Einheiten gelisteter Makros
  folgen jeder Datei ihres Pakets; ein ungelistetes Makro, das eine Datei liest,
  ohne sie bei rustc zu registrieren, kann veraltete Ergebnisse liefern – genau wie
  bei Cargo.
- **Build-Skripte laufen nicht in einer Sandbox.** Ein Skript, das Dateien oder Variablen liest,
  die es nicht mit `rerun-if-*` deklariert hat, kann veraltete Ergebnisse liefern – genau wie bei
  Cargo.
- **Nicht unterstützte Cargo-Features schlagen unmissverständlich fehl.** Zum Beispiel `--profile`,
  Globs mit Wildcards in Zwischenverzeichnissen in `workspace.members`. Mitts
  meldet diese Fehler explizit, anstatt Vermutungen anzustellen.
- **Zero-Copy auf demselben Dateisystem.** Zero-Copy-Einheiten-Promotion aus dem Staging setzt voraus,
  dass `MITTS_CACHE_DIR` und das `target/`-Verzeichnis des Workspace auf demselben
  Dateisystem-Mount liegen. Beim Überschreiten von Dateisystemgrenzen greift das System sicher auf standardmäßiges Kopieren zurück.

**Verwenden Sie Mitts noch nicht**, wenn Sie Windows, Cross-Kompilierung, einen Team-Cache oder
ein stabiles Release benötigen. Cargo (optional mit sccache) ist in diesen Fällen die sichere Wahl.

<a id="performance"></a>
## Leistung

Gemessen mit Mitts `0.3.17` über **100 Open-Source-Rust-Projekte** unter macOS (Apple M2 Ultra, APFS) und Linux ARM64 (XFS) sowie **90** unter Linux x86_64 (AMD Ryzen 9 9950X, XFS) im Vergleich zu reinem `cargo` und `cargo` mit `sccache` (`0.17.0`). Jeder Messwert ist der Median aus 3 Durchläufen mit geprüfter Parität von Diagnosen, Exit-Codes und Testergebnissen; alle gemessenen Projekte bleiben in den Gesamtsummen enthalten (`0`, `7` und `0` als verrauscht markierte Läufe), und kein Zahlenwert übertrifft die Messung: Vergleichswerte werden abgeschnitten und Mitts wird aufgerundet ([ROADMAP M8](../ROADMAP.md)). Die vollständige Studie, jedes einzelne Projekt und die Methodik finden sich unter [mittsbuild.com/the-hundred](https://mittsbuild.com/the-hundred/).

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="../docs/img/bench-scenarios-dark.svg">
    <source media="(prefers-color-scheme: light)" srcset="../docs/img/bench-scenarios-light.svg">
    <img src="../docs/img/bench-scenarios-light.svg" width="920" alt="Build-Zeit des Medianprojekts über sechs Szenarien: Cargo, Cargo + sccache und Mitts">
  </picture>
</p>

Jede Zeile zeigt die Build-Zeit des Medianprojekts (der jeweils eigene Median jedes Werkzeugs über die Projekte, lineare Skala pro Zeile), daneben die Anzahl der Projekte, bei denen Mitts als Erstes fertig war. Unter macOS baut das Medianprojekt seine 12 Worktrees in **32 s** mit `cargo`, **31 s** mit `cargo` + `sccache` und **3.8 s** mit Mitts. Mit warmem lokalem Store benötigt ein zweiter sauberer Worktree **1.7 s**, **1.6 s** und **79 ms**; ein Branch-Wechsel **627 ms**, **209 ms** und **27 ms**; ein Build ohne Änderungen **100 ms**, **41 ms** und **9 ms**. Mitts ist in diesen vier Szenarien bei 99–100 von 100 Projekten als Erstes fertig (71 von 71 beim zweiten Worktree).

Wo Mitts nicht gewinnt:

- **Kaltstart-Build, leerer Cache.** Reines `cargo` ist bei 71 von 100 macOS-Projekten als Erstes fertig (60 von 90 unter Linux x86_64, 77 von 100 unter Linux aarch64), bei den größten um bis zu 6.2 s, weil Mitts die Eingaben hasht und einen leeren Store befüllt. Das Medianprojekt benötigt **1.7 s** mit `cargo` und **2.0 s** mit Mitts. Gegenüber einem kalten `sccache` liegt Mitts vorn (**2.4 s**, Mitts bei 86 von 100 vorn).
- **Eine Datei bearbeiten.** Das Medianprojekt benötigt **364 ms** mit `cargo`, **596 ms** mit `sccache` und **315 ms** mit Mitts, wobei Mitts bei 49 von 71 Projekten vor `cargo` liegt. Bei den übrigen 22 Projekten (21 von 64 auf x86_64, 29 von 71 auf aarch64) ist `cargo` um bis zu 2.6 s unter macOS und 8.6 s auf x86_64 schneller, weil es die inkrementelle Kompilierung von `rustc` innerhalb eines großen Crates wiederverwendet, während Mitts ganze Crate-Einheiten cacht.
- **Abspielen von 10 Commits** (nicht im Diagramm): geteiltes Bild, Mitts liegt unter macOS bei 58 von 100 Projekten vorn, bei nahe beieinanderliegenden Medianen (**5.6 s** mit `cargo`, **5.8 s** mit Mitts).

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="../docs/img/bench-swarm-dark.svg">
    <source media="(prefers-color-scheme: light)" srcset="../docs/img/bench-swarm-light.svg">
    <img src="../docs/img/bench-swarm-light.svg" width="920" alt="12-Worktree-Build-Scoreboard über 100 Rust-Projekte: Zeit, CPU, Compiler-Läufe und geschriebene Datenmenge">
  </picture>
</p>

Paralleles Bauen von **12 Worktrees** auf 4 Commits über alle Projekte, Gesamtsummen pro Maschine:

| Maschine | Zeit: `cargo` / `sccache` / Mitts | `rustc`-Läufe: `cargo` / Mitts | Geschriebene Daten: `cargo` / Mitts |
|---|---|---|---|
| macOS, APFS, M2 Ultra (100 Projekte) | 76 min / 73 min / **13 min** | 90,961 / **6,905** | 641 GB / **112 GB** |
| Linux x86_64, XFS, Ryzen 9 9950X (90 Projekte) | 60 min / 52 min / **9 min** | 75,049 / **6,162** | 651 GB / **106 GB** |
| Linux aarch64, XFS (100 Projekte) | 78 min / 76 min / **22 min** | 90,544 / **7,348** | 743 GB / **121 GB** |

Die CPU-Zeit unter macOS sinkt von **20.3 h** (`cargo`) und **8.0 h** (`sccache`) auf **2.7 h**. Auf der x86_64-Maschine wurden 90 statt 100 Projekte gemessen: Zehn Projekte fehlen in diesem Durchlauf (similar, smallvec, smol, syntect, tealdeer, tera, textwrap, tokei, tower-http, xh), und die Ursache ist nicht dokumentiert.


## Status

| Bereich | Zustand |
| --- | --- |
| Lesen von `Cargo.toml` / `Cargo.lock`, Sparse-Index-Auflösung | erledigt |
| Feature-Auflösung (Resolver 1 und 2) | erledigt |
| Paralleler Downloader und globaler Quellcode-Speicher | erledigt |
| Nativer DAG-Runner, pipelined `rustc`, Jobserver | erledigt |
| Inhaltsadressierter Speicher und Action-Cache | erledigt |
| Ausführung und Caching von Build-Skripten | erledigt |
| Schneller Pfad bei No-Op | erledigt |
| `check`, `clippy`, `fmt`, `test` (inkl. Doctests), `coverage` | erledigt |
| Konservative GC mit Quoten | erledigt |
| C/C++-Adapter auf demselben Speicher | experimentell |
| Pfadunabhängige Schlüssel für Workspace-Crates | teilweise |
| Reproduzierbare Benchmark-Studie | erledigt |
| Remote- / Team-Cache | geplant |
| Windows, Cross-Kompilierung | nicht unterstützt |

Meilensteine und Nachweise: [ROADMAP.md](../ROADMAP.md).

## FAQ

**Ersetzt Mitts Cargo?**
Mitts liest die vorhandenen Cargo-Manifeste und Lock-Dateien und stellt den Rust-Build-
und Test-Workflow für dieses Projekt bereit. Seine eigenen CI- und Release-Workflows nutzen Mitts,
ohne Cargo aufzurufen. Cargo bleibt für externe Anwender, die es benötigen, weiterhin verfügbar.

**Kann ich es in der CI verwenden?**
Ja. Die CI dieses Repositories installiert das neueste stabile Mitts und nutzt es anschließend für
native Builds, Tests, Coverage, Formatierung und Clippy. Der geteilte Artefakt-Speicher
ist lokal auf jedem Runner; ein maschinenübergreifender Remote-Cache ist derzeit noch nicht verfügbar.

**Kollidiert es mit rust-analyzer oder vorhandenen Cargo-Ausgaben?**
Mitts nutzt dieselben Standard-Ausgabepfade wie Cargo. Sein geteilter Artefakt-Speicher bleibt
getrennt, und `mitts clean` entfernt ausschließlich indexierte Mitts-Ausgaben, die noch mit
ihrem aufgezeichneten Inhalt übereinstimmen; es bewahrt unbeteiligte oder ersetzte Cargo-Dateien.
Hält Cargo die Sperre des Build-Verzeichnisses eines Profils, wird das Bereinigen übersprungen, bis dieser
Build abgeschlossen ist.

**Wo liegt der Cache und wie leere ich ihn?**
`--cache-dir`, sonst `$MITTS_CACHE_DIR`, `$XDG_CACHE_HOME/mitts` oder
`~/.cache/mitts`; alles, was Mitts cacht, liegt unter diesem einen Stammverzeichnis. Nutzen Sie
`mitts gc`, um innerhalb der Quoten aufzuräumen; das Verzeichnis zu löschen ist ebenfalls sicher, wenn kein
Build aktiv ist.

**Warum wurde eine Einheit nicht zwischengespeichert?**
Führen Sie `mitts explain` aus. Es listet jede Einheit und die Begründung auf, warum sie für den Cache
infrage kam oder nicht.

## Designregeln

- Build- und Cache-Orchestrierung bleiben sprachneutral. Die Sprachsemantik liegt
  in Adaptern; Rust ist der erste.
- Ein Cache-Treffer erfordert vollständige, erfasste Eingaben. Unbekannte Eingaben werden wie
  geänderte Eingaben behandelt.
- Geteilte Artefakte sind unveränderlich und werden vor der Verwendung verifiziert. Veränderlicher Zustand verbleibt
  in jedem Workspace. Keine Hardlinks auf beschreibbare Dateien, keine Zeitstempeltricks.
- Bereinigungsvorgänge rühren niemals Artefakte eines aktiven Builds an.

Regeln für Beitragende und Agenten: [AGENTS.md](../AGENTS.md).
Architektur-Entscheidungen: [docs/decisions](../docs/decisions).

## Autor

[Alexander Panasenko](https://prod.codes/about/) · alex@prod.codes ·
[mittsbuild.com](https://mittsbuild.com)

## Zitierung

Zum Zitieren von Mitts verwenden Sie die archivierte Version. Der Konzept-DOI [10.5281/zenodo.24030067](https://doi.org/10.5281/zenodo.24030067) verweist immer auf die neueste Version; der Versions-DOI von 0.3.36 ist [10.5281/zenodo.24213821](https://doi.org/10.5281/zenodo.24213821). Die GitHub-Funktion „Cite this repository“ liest [CITATION.cff](../CITATION.cff).

```bibtex
@software{panasenko_2026_24213821,
  author    = {Panasenko, Alexander},
  title     = {Mitts Build — Rust-first build system with a machine-wide, content-addressed artifact cache},
  year      = {2026},
  publisher = {Zenodo},
  version   = {0.3.36},
  doi       = {10.5281/zenodo.24213821},
  url       = {https://doi.org/10.5281/zenodo.24213821}
}
```

## Lizenz

Doppelt lizenziert unter [MIT](../LICENSE-MIT) ODER [Apache-2.0](../LICENSE-APACHE), nach Ihrer Wahl.
