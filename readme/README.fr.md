<!-- translation-source: 1a926080b49fae712d44fdae44fb6f96b80d3499 -->

> [!NOTE]
> Ceci est une traduction de la documentation officielle en anglais au commit [1a92608](https://github.com/alex09x/mitts/blob/1a926080b49fae712d44fdae44fb6f96b80d3499/README.md). La version canonique et la plus à jour reste le [README en anglais](https://github.com/alex09x/mitts/blob/main/README.md).

<p align="center">
  <img src="../site/logo.svg" width="96" height="96" alt="Mitts Build logo">
</p>

<h1 align="center">Mitts Build</h1>

<p align="center">
  <b>Le travail de compilation, pris en compte.</b><br>
  Un système de build pensé d'abord pour Rust avec un cache d'artefacts adressé par le contenu,
  à l'échelle de la machine, et un suivi explicite des entrées. Il lit vos fichiers <code>Cargo.toml</code> et <code>Cargo.lock</code> existants.
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
  <a href="#quick-start">Démarrage rapide</a> ·
  <a href="#how-it-works">Fonctionnement</a> ·
  <a href="#architecture">Architecture</a> ·
  <a href="../docs/architecture.md">Protocole</a> ·
  <a href="#limitations">Limitations</a> ·
  <a href="../ROADMAP.md">Feuille de route</a>
</p>

<p align="center">
  <a href="https://github.com/alex09x/mitts/blob/main/README.md">English</a> ·
  <a href="https://github.com/alex09x/mitts/blob/main/readme/README.ru.md">Русский</a> ·
  <a href="https://github.com/alex09x/mitts/blob/main/readme/README.zh-CN.md">简体中文</a> ·
  <a href="https://github.com/alex09x/mitts/blob/main/readme/README.es.md">Español</a> ·
  <a href="https://github.com/alex09x/mitts/blob/main/readme/README.pt-BR.md">Português (Brasil)</a> ·
  <a href="https://github.com/alex09x/mitts/blob/main/readme/README.ja.md">日本語</a> ·
  <a href="https://github.com/alex09x/mitts/blob/main/readme/README.de.md">Deutsch</a> ·
  <a href="https://github.com/alex09x/mitts/blob/main/readme/README.fr.md"><b>Français</b></a> ·
  <a href="https://github.com/alex09x/mitts/blob/main/readme/README.ko.md">한국어</a> ·
  <a href="https://github.com/alex09x/mitts/blob/main/readme/README.ar.md">العربية</a>
</p>

---

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="../docs/img/bench-hero-dark.svg">
    <source media="(prefers-color-scheme: light)" srcset="../docs/img/bench-hero-light.svg">
    <img src="../docs/img/bench-hero-light.svg" width="920" alt="Compiler 12 worktrees de 100 projets Rust sur un Mac prend 13 minutes avec Mitts et 76 minutes avec Cargo ; les exécutions du compilateur passent de 90,961 à 6,905 et les écritures disque de 641 GB à 112 GB">
  </picture>
</p>

<p align="center">
  <sub>100 projets open source (90 sous Linux x86_64), 12 worktrees chacun, compilés simultanément sur trois machines. Cargo et Cargo + sccache comme références, y compris les cas où Mitts n'est pas plus rapide : <a href="#performance">Performances</a> · <a href="https://mittsbuild.com/the-hundred/">The Hundred</a></sub>
</p>

Mitts planifie lui-même l'espace de travail et invoque directement `rustc`. Chaque
action de compilation reçoit une clé dérivée de l'ensemble de ses entrées. Le résultat est stocké
une seule fois dans un magasin immuable à l'échelle de la machine et restauré dans chaque espace de travail
qui en a besoin, à travers les projets, les worktrees et les branches.

Si une entrée ne peut pas être prise en compte, l'action est recompilée, jamais devinée.

## Le problème

Le cache de build de Cargo réside dans le répertoire `target/` de chaque espace de travail. Tout
ce qui crée un nouveau `target/` repart de zéro :

- un second checkout ou `git worktree` du même dépôt,
- basculer sur une branche qui modifie `Cargo.lock`, puis revenir en arrière,
- un autre projet qui dépend des mêmes crates avec les mêmes fonctionnalités (features),
- des agents de codage parallèles, travaillant chacun dans son propre worktree.

Le même `serde`, `syn` ou `tokio` est compilé encore et encore, et chaque copie
consomme de l'espace disque.

Les solutions de contournement habituelles présentent des inconvénients majeurs :

- **Un `CARGO_TARGET_DIR` partagé** sérialise les builds derrière un verrou unique, et
  les projets avec des ensembles de fonctionnalités différents écrasent mutuellement leurs artefacts.
- **Un wrapper de compilateur tel que sccache** voit un appel à `rustc` à la fois. Sa
  clé dépend des chemins absolus à moins qu'ils ne soient normalisés, et les crates qui
  invoquent l'éditeur de liens (`bin`, `proc-macro`, `dylib`) ne sont pas mises en cache. Cargo planifie
  toujours le build et exécute chaque script de build.

Mitts emprunte une voie différente : il remplace l'exécuteur de Cargo, pas le compilateur.
Il voit l'ensemble du graphe d'unités, ce qui lui permet de calculer la clé de chaque unité à partir des clés de ses dépendances,
de mettre en cache les unités d'édition de liens et les exécutions de scripts de build, et de permettre à plusieurs builds
de partager un même magasin en toute sécurité.

## Points clés

- **Réutilisation entre espaces de travail.** Un magasin par machine. Une dépendance compilée pour
  un checkout est restaurée dans le suivant avec les mêmes entrées.
- **Clés complètes.** Chaîne d'outils, cible, profil, drapeaux, fonctionnalités,
  contenu des sources, clés des dépendances et environnement suivi. Des entrées inconnues entraînent
  une recompilation.
- **Les manifestes Cargo restent le contrat.** Pas de nouveau manifeste, pas de second lockfile.
  Les flux de travail de build, test, lint, couverture et code source de publication de ce dépôt s'exécutent
  via Mitts ; ils n'invoquent pas l'interface CLI de Cargo. L'interopérabilité externe avec Cargo
  reste accessible aux utilisateurs.
- **Les unités d'édition de liens et les scripts de build sont mis en cache.** Les binaires, tests, macros procédurales
  et exécutions de scripts de build sont associés à une clé et stockés, pas seulement les `rlib`s.
- **Les builds concurrents coopèrent.** Verrous par clé : deux terminaux ou worktrees
  ayant besoin de la même dépendance la compilent une seule fois. Un jobserver unique limite
  le parallélisme pour tous les processus Mitts sur la machine.
- **Partage sécurisé.** Les objets du magasin sont immuables : publiés de manière atomique, stockés
  en lecture seule et restaurés par reflink ou copie. `MITTS_VERIFY_RESTORE=1` recalcule le hachage de chaque
  objet avant sa restauration (le comportement par défaut fait confiance au magasin, ce qui économise une lecture de
  chaque octet restauré). Lorsque le système de fichiers ne supporte pas le reflink, la restauration crée un lien matériel (hardlink) de
  l'objet en lecture seule du magasin dans `target/` au lieu de copier ses octets : l'inœud partagé
  reste `0o444` (`0o555` pour les exécutables), l'index de la cible rejette tout lien ayant acquis
  des bits d'écriture, et Mitts ne remplace jamais les fichiers de l'espace de travail que par renommage. Jamais de lien
  accessible en écriture vers un objet du magasin. `MITTS_STORE_HARDLINKS=0` conserve des copies privées pour les espaces de travail
  dans lesquels d'autres outils écrivent sur place.
- **Staging d'espace de travail sans copie.** Les alias de bibliothèque au sein de l'espace de travail, les binaires secondaires
  et le staging atomique des unités utilisent des liens matériels internes à l'espace de travail et une promotion par renommage sans
  copier d'octets. Lorsque `MITTS_CACHE_DIR` et le `target/` de l'espace de travail résident sur le même
  volume de système de fichiers, les promotions de staging fonctionnent sans copier d'octets ; entre des points de
  montage séparés, elles basculent de manière transparente vers une copie sécurisée.
- **Un chemin rapide sans opération (no-op fast path).** Un espace de travail inchangé saute la planification et le calcul des clés.
- **Une seule interface CLI.** `build`, `run`, `check`, `clippy`, `fmt`, `test` (avec doctests),
  `coverage`, `explain`, `fetch`, `gc`, `clean`.

<a id="quick-start"></a>
## Démarrage rapide

Installez la dernière version native signée :

```sh
curl -fsSL https://mittsbuild.com/install.sh | sh
```

Pour compiler le code source cloné, installez d'abord la dernière version distribuée de Mitts, puis utilisez
Mitts pour compiler le dépôt :

```sh
git clone https://github.com/alex09x/mitts
cd mitts
mitts build --release
MITTS_JOBS=2 mitts build     # limit concurrent native build units
install -m755 target/release/mitts "$HOME/.cargo/bin/mitts"
```

Exécutez-le ensuite au sein de n'importe quel espace de travail Cargo :

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

Pour les projets Git, la création de paquets nécessite actuellement un dépôt propre avec le paquet à sa racine.
Les motifs `include` du manifeste peuvent sélectionner des fichiers ignorés ; les paquets imbriqués dans l'espace de travail
et les répertoires sous forme de liens symboliques ne sont pas encore pris en charge. `mitts publish` cible crates.io et lit
le jeton depuis `CARGO_REGISTRY_TOKEN`.

Mitts écrit les résultats de compilation dans les répertoires habituels de Cargo, `target/debug/` et `target/release/`.
`CARGO_TARGET_DIR` et `--target-dir` permettent de sélectionner une racine de sortie différente.
Son cache partagé est distinct
(`--cache-dir`, sinon `$MITTS_CACHE_DIR`, sinon `$XDG_CACHE_HOME/mitts`, sinon
`~/.cache/mitts`). Le magasin d'artefacts, le cache d'actions, les verrous, le jobserver, le magasin
de sources, les miroirs git et le cache d'index résident tous sous cette racine unique.

Les dépendances Git, publiques ou privées, sont clonées par Mitts lui-même avec le `git`
système, de sorte que vos clés SSH, agent, gestionnaires d'identifiants et `GIT_SSH_COMMAND` s'appliquent
exactement comme en ligne de commande ; les invites interactives sont désactivées, ainsi un identifiant manquant
échoue au lieu de bloquer indéfiniment. Rien ne passe par Cargo. Un clone que Cargo a déjà
effectué sous `~/.cargo/git/checkouts` est réutilisé au lieu d'être cloné à nouveau.

Toutes les autres façons dont Cargo obtient une crate fonctionnent de la même manière, à partir de la même configuration :

- **Registres alternatifs** : `[registries.<name>] index = "sparse+https://…"`
  et `registry = "<name>"` sur la dépendance. Les jetons proviennent des fournisseurs
  d'identifiants de Cargo, dans l'ordre de Cargo : `cargo:token` (l'environnement et
  `credentials.toml`), `cargo:macos-keychain`, `cargo:libsecret`,
  `cargo:token-from-stdout <command>`, et des greffons tels que
  `cargo-credential-1password` via le protocole des fournisseurs d'identifiants,
  configurés par `[registry] global-credential-providers`,
  `[registries.<name>] credential-provider` et `[credential-alias]`. Un registre
  dont le `config.json` indique `auth-required` échoue avec un message clair si aucun
  fournisseur ne dispose d'un jeton. Les registres basés sur un index Git sont refusés : utilisez le protocole
  sparse.
- **Miroirs** : `[source.crates-io] replace-with = "<name>"` avec
  `[source.<name>] registry = "sparse+https://…"`. Les octets proviennent du miroir,
  `Cargo.lock` continue de nommer crates.io, exactement comme Cargo l'écrit.
- **Sources vendorisées** : `[source.vendored-sources] directory = "vendor"` issu de
  `cargo vendor`. Les paquets sont vérifiés par rapport à leur `.cargo-checksum.json`
  et copiés dans le magasin ; rien ne transite par le réseau, paquets
  git inclus. En l'absence de lockfile, le répertoire vendor fait office d'index.
- **`[patch]`** : `[patch.crates-io] foo = { path = … }` ou `{ git = … }`
  remplace le paquet du registre ; sans lockfile, le remplacement est
  résolu et épinglé, et il doit satisfaire la contrainte, comme avec Cargo.

La section `[env]` d'une configuration Cargo s'applique à rustc, aux scripts de build et aux programmes
lancés par `mitts run` et `mitts test`, selon les règles `force` et `relative` de Cargo ;
les fichiers de configuration étant des entrées, toute valeur modifiée entraîne la recompilation de ce qui l'a lue.

Sans `Cargo.lock`, Mitts en résout un de la même manière que Cargo : version non retirée
(non-yanked) la plus élevée par contrainte, propagation des fonctionnalités le long des arêtes, dépendance
optionnelle incluse uniquement lorsqu'une fonctionnalité l'active, une version par
plage de compatibilité. Le résultat est un lockfile que Cargo accepte comme le sien.

<a id="how-it-works"></a>
## Fonctionnement

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

La clé d'une unité inclut les **clés** de ses dépendances, pas leurs octets de sortie, de sorte qu'une
modification n'importe où en dessous d'une unité change sa clé. Les scripts de build s'exécutent dans un répertoire
de staging ; leurs directives (`rustc-cfg`, `rustc-link-*`, `rustc-env`, …) entrent dans
la clé de l'unité qu'ils configurent, et leurs déclarations `rerun-if-*` déterminent
quand une exécution en cache peut être réutilisée.

Après un build réussi, Mitts enregistre un instantané de l'espace de travail. Si rien
n'y a changé, le build suivant passe directement au résultat. Tout écart entraîne
un repli vers le build complet avec calcul des clés, ainsi le chemin rapide ne saute jamais de travail que le protocole
effectuerait.

Le protocole complet — dérivation des clés, verrouillage, scripts de build, chemin rapide,
organisation du magasin et ramasse-miettes — se trouve dans
[docs/architecture.md](../docs/architecture.md).

<a id="architecture"></a>
## Architecture

Mitts est un cœur indépendant du langage sur lequel reposent des adaptateurs de langage. Le cœur ne sait
rien de Cargo ; l'adaptateur Rust transforme un espace de travail Cargo en actions
associées à des clés.

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

| Chemin | Contenu |
| --- | --- |
| `src/cli/` | Gestionnaires de commandes : `build`, `test`, `clippy`, `explain`, … |
| `src/adapter/rust/` | Sémantique de Cargo : manifestes, lockfile, index sparse, résolution des fonctionnalités, planification des unités, clés d'action, exécuteur de DAG, scripts de build, doctests |
| `src/adapter/c/` | Adaptateur C/C++ expérimental sur le même magasin |
| `src/core/` | Magasin indépendant du langage (`cas.rs`), cache d'actions, verrous, jobserver, chemin rapide (`build_lock*.rs`), ramasse-miettes, reflinks |
| `docs/` | [Protocole](../docs/architecture.md), [décisions de conception](../docs/decisions), [scénarios](../docs/scenarios) |
| `tests/` | Tests d'intégration, y compris parité avec Cargo et invalidation du cache |
| `scripts/` | Bancs d'essai et infrastructure d'audit |
| `fixtures/` | Projets minimaux Rust et C utilisés par les tests |

## Ce qui invalide le cache

Un succès de cache (cache hit) nécessite la correspondance de chaque entrée ci-dessous. Si Mitts ne peut pas observer une
entrée, il ne réutilise pas le résultat.

| Entrée | Suivie en tant que |
| --- | --- |
| Chaîne d'outils | Sortie de `rustc -vV` |
| Cible et profil | Triplet cible, paramètres de profil, `RUSTFLAGS` et rustflags de configuration Cargo |
| Fonctionnalités | Ensemble des fonctionnalités résolues de l'unité (résolveur 1 ou 2, comme Cargo) |
| Sources | Hachages de contenu des fichiers du paquet et de chaque fichier dont rustc signale la lecture (`include_str!`, `include_bytes!`, modules) |
| Dépendances | Clés d'action de toutes les unités de dépendance |
| Environnement | Paramètres `CARGO_*` et `RUST_*` pertinents, et variables lues via `env!` / `option_env!`, avec `<unset>` consigné explicitement |
| Configuration Cargo | Fichiers `.cargo/config(.toml)` que Cargo lirait |
| Scripts de build | Leurs directives, `OUT_DIR`, et le contenu des chemins `rerun-if-changed` ainsi que les valeurs des variables `rerun-if-env-changed` |

Les programmes configurés `rustc-wrapper` et `rustc-workspace-wrapper` sont invoqués pour
la compilation. Comme un wrapper arbitraire peut dépendre d'entrées non suivies, Mitts
désactive la réutilisation du cache d'actions et du cache de scripts de build lorsque l'un ou l'autre de ces wrappers est actif.
Cela peut ralentir ces builds.

Certaines macros procédurales lisent des fichiers de votre paquet lors de leur expansion : `sqlx` vérifie
les requêtes par rapport à `.sqlx/`, `askama` compile `templates/`, `rust-embed` et
`include_dir` intègrent des répertoires entiers. rustc n'est informé de ces lectures que lorsque
la macro les enregistre, il faut donc déclarer ces crates dans le `Cargo.toml` racine :

```toml
[workspace.metadata.mitts]          # [package.metadata.mitts] in a single crate
impure-proc-macros = ["sqlx", "askama", "rust-embed"]
```

Les crates qui dépendent d'une macro listée ont pour clé chaque fichier de leur paquet
et leur chemin absolu : un gabarit modifié entraîne leur recompilation, et un autre
checkout ne les réutilise jamais. Toute autre macro est traitée comme pure, et ses
dépendants sont partagés entre les worktrees. Nommez la crate dont vous dépendez ; la
crate de macro qu'elle importe (`sqlx-macros`) fonctionne également.

## Comparaison

Une comparaison qualitative de la manière dont chaque outil réutilise le travail. Il ne s'agit pas d'une
affirmation de performance.

| | Cargo | Cargo + sccache | Mitts |
| --- | --- | --- | --- |
| Lit `Cargo.toml` / `Cargo.lock` tel quel | oui | oui | oui |
| Réutilisation entre espaces de travail et worktrees | non (par `target/`) | oui, par appel à `rustc` | oui, par unité |
| La clé dépend du chemin du checkout | — | oui, sauf si normalisé | non pour les crates de registre ; pour les crates d'espace de travail seulement si non éligibles |
| Met en cache les unités `bin`, de test et `proc-macro` | — | non | oui |
| Met en cache les exécutions de scripts de build | non | non | oui |
| Compilations concurrentes d'une même dépendance | attente sur le verrou de `target/` | les deux peuvent compiler en cas d'absence | l'un compile, l'autre réutilise |
| Cache distant / d'équipe | non | oui (S3, GCS, Redis, …) | prévu |
| Windows | oui | oui | non |

Bazel et Buck2 résolvent un problème plus vaste — des builds hermétiques, multilingues avec
exécution distante — mais nécessitent leurs propres fichiers de build au lieu de `Cargo.toml`.

<a id="limitations"></a>
## Limitations

Mitts est en version préliminaire. Sachez ceci avant de vous appuyer dessus :

- **Linux et macOS uniquement.** Les verrous et le jobserver sont POSIX.
- **Cible hôte uniquement.** La compilation croisée n'est pas encore prise en charge.
- **Pas encore de cache distant.** Le magasin est propre à chaque machine.
- **Registres sparse uniquement.** Un registre fourni sous forme d'index git (`registry+…`
  autre que crates.io, ou `[source] registry = "https://…"` sans
  `sparse+`) et les sources `local-registry` sont refusés avec des conseils.
- **Les identifiants sont lus, jamais écrits.** `cargo login` reste le rôle de Cargo ;
  `cargo:wincred` est réservé à Windows et ignoré.
- **Certaines unités ne sont jamais mises en cache :** les crates de type `cdylib`, `dylib` ou
  `staticlib`, et les crates dont les lectures de `env!` ne peuvent pas être suivies. Elles sont toujours
  compilées ; elles sont signalées par `mitts explain`.
- **Crates d'espace de travail entre checkouts.** Vos propres crates ne sont réutilisées entre
  worktrees que lorsque leur clé peut être rendue indépendante du chemin (pas de
  lecture de `CARGO_MANIFEST_DIR`, pas de script de build, pas de macro procédurale impure déclarée).
  Sinon, la clé inclut le chemin absolu. Sur macOS, les stabs de débogage de l'éditeur de liens
  sont écrits de manière relative à l'espace de travail (`-oso_prefix`), de sorte que les binaires liés
  sont également partagés ; démarrez le débogueur à la racine de l'espace de travail pour qu'il
  trouve les fichiers objets.
- **Les macros qui lisent vos fichiers doivent être déclarées par vos soins.** Une macro procédurale est traitée
  comme pure sauf si le projet la répertorie (voir ci-dessus). Les dépendants d'une macro répertoriée
  suivent chaque fichier de leur paquet ; une macro non répertoriée qui lit un fichier
  sans l'enregistrer auprès de rustc peut produire un résultat obsolète, exactement comme
  avec Cargo.
- **Les scripts de build ne sont pas isolés en bac à sable (sandbox).** Un script qui lit des fichiers ou des variables qu'il
  n'a pas déclarés avec `rerun-if-*` peut produire un résultat obsolète — exactement comme avec
  Cargo.
- **Les fonctionnalités Cargo non prises en charge échouent explicitement.** Par exemple `--profile`, les motifs génériques (globs)
  avec caractères génériques dans les répertoires intermédiaires de `workspace.members`. Mitts
  les signale au lieu de deviner.
- **Zéro-copie sur le même système de fichiers.** La promotion d'unités sans copie depuis le staging nécessite que
  `MITTS_CACHE_DIR` et le répertoire `target/` de l'espace de travail résident sur le même
  point de montage de système de fichiers. Le franchissement des limites de systèmes de fichiers bascule de façon sécurisée vers une copie standard.

**N'utilisez pas encore Mitts** si vous avez besoin de Windows, de compilation croisée, d'un cache d'équipe ou
d'une version stable. Cargo, éventuellement avec sccache, est le choix sûr dans ces cas.

<a id="performance"></a>
## Performances

Mesuré avec Mitts `0.3.17` sur **100 projets Rust open source** sous macOS (Apple M2 Ultra, APFS) et Linux ARM64 (XFS), et **90** sous Linux x86_64 (AMD Ryzen 9 9950X, XFS), face à `cargo` seul et `cargo` avec `sccache` (`0.17.0`). Chaque mesure correspond à la médiane de 3 répétitions avec vérification de parité des diagnostics, codes de sortie et résultats de tests ; tous les projets mesurés restent inclus dans les totaux (`0`, `7` et `0` exécutions signalées bruyantes), et aucun chiffre ne dépasse la mesure : les références sont tronquées et Mitts est arrondi à la hausse ([ROADMAP M8](../ROADMAP.md)). L'étude complète, chaque projet et la méthode sont sur [mittsbuild.com/the-hundred](https://mittsbuild.com/the-hundred/).

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="../docs/img/bench-scenarios-dark.svg">
    <source media="(prefers-color-scheme: light)" srcset="../docs/img/bench-scenarios-light.svg">
    <img src="../docs/img/bench-scenarios-light.svg" width="920" alt="Temps de compilation du projet médian sur six scénarios : Cargo, Cargo + sccache et Mitts">
  </picture>
</p>

Chaque ligne représente le temps de compilation du projet médian (la propre médiane de chaque outil sur les projets, échelle linéaire par ligne), avec à côté le nombre de projets sur lesquels Mitts a terminé en premier. Sous macOS, le projet médian compile ses 12 worktrees en **32 s** avec `cargo`, **31 s** avec `cargo` + `sccache` et **3.8 s** avec Mitts. Avec un magasin local déjà chaud, un second worktree propre prend **1.7 s**, **1.6 s** et **79 ms** ; un changement de branche **627 ms**, **209 ms** et **27 ms** ; une compilation sans changement **100 ms**, **41 ms** et **9 ms**. Mitts arrive en tête sur 99–100 des 100 projets dans ces quatre scénarios (71 sur 71 pour le second worktree).

Là où Mitts ne gagne pas :

- **Build à froid, cache vide.** `cargo` seul termine en premier sur 71 des 100 projets macOS (60 sur 90 sous Linux x86_64, 77 sur 100 sous Linux aarch64), jusqu'à 6.2 s plus vite sur les plus gros, car Mitts hache les entrées et remplit un magasin vide. Le projet médian prend **1.7 s** avec `cargo` et **2.0 s** avec Mitts. Mitts devance un `sccache` à froid (**2.4 s**, Mitts premier sur 86 des 100).
- **Modification d'un fichier.** Le projet médian prend **364 ms** avec `cargo`, **596 ms** avec `sccache` et **315 ms** avec Mitts, et Mitts termine en premier sur 49 des 71 projets face à `cargo`. `cargo` est premier sur les 22 autres (21 sur 64 sur x86_64, 29 sur 71 sur aarch64), jusqu'à 2.6 s plus vite sous macOS et 8.6 s sur x86_64, parce qu'il réutilise la compilation incrémentale de `rustc` à l'intérieur d'un gros crate alors que Mitts met en cache des unités de crate entières.
- **Rejeu de 10 commits** (hors graphique) : résultat partagé, Mitts termine en premier sur 58 des 100 projets sous macOS avec des médianes proches (**5.6 s** avec `cargo`, **5.8 s** avec Mitts).

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="../docs/img/bench-swarm-dark.svg">
    <source media="(prefers-color-scheme: light)" srcset="../docs/img/bench-swarm-light.svg">
    <img src="../docs/img/bench-swarm-light.svg" width="920" alt="Tableau de bord de compilation sur 12 worktrees pour 100 projets Rust : temps, CPU, exécutions du compilateur et écritures disque">
  </picture>
</p>

Compilation de **12 worktrees en parallèle** sur 4 commits sur l'ensemble des projets, totaux par machine :

| Machine | Temps : `cargo` / `sccache` / Mitts | Exécutions `rustc` : `cargo` / Mitts | Écritures disque : `cargo` / Mitts |
|---|---|---|---|
| macOS, APFS, M2 Ultra (100 projets) | 76 min / 73 min / **13 min** | 90,961 / **6,905** | 641 GB / **112 GB** |
| Linux x86_64, XFS, Ryzen 9 9950X (90 projets) | 60 min / 52 min / **9 min** | 75,049 / **6,162** | 651 GB / **106 GB** |
| Linux aarch64, XFS (100 projets) | 78 min / 76 min / **22 min** | 90,544 / **7,348** | 743 GB / **121 GB** |

Le temps CPU sous macOS passe de **20.3 h** (`cargo`) et **8.0 h** (`sccache`) à **2.7 h**. La machine x86_64 a mesuré 90 projets et non 100 : dix projets manquent à cette exécution (similar, smallvec, smol, syntect, tealdeer, tera, textwrap, tokei, tower-http, xh), sans que la cause soit enregistrée.


## État d'avancement

| Domaine | État |
| --- | --- |
| Lecture de `Cargo.toml` / `Cargo.lock`, résolution d'index sparse | terminé |
| Résolution des fonctionnalités (résolveurs 1 et 2) | terminé |
| Récupération parallèle et magasin global de sources | terminé |
| Exécuteur de DAG natif, `rustc` en pipeline, jobserver | terminé |
| Magasin adressé par le contenu et cache d'actions | terminé |
| Exécution et mise en cache des scripts de build | terminé |
| Chemin rapide sans opération (no-op fast path) | terminé |
| `check`, `clippy`, `fmt`, `test` (incluant doctests), `coverage` | terminé |
| Ramasse-miettes conservateur avec quotas | terminé |
| Adaptateur C/C++ sur le même magasin | expérimental |
| Clés indépendantes du chemin pour les crates d'espace de travail | partiel |
| Étude de bancs d'essai reproductible | terminé |
| Cache distant / d'équipe | prévu |
| Windows, compilation croisée | non pris en charge |

Jalons et éléments de preuve : [ROADMAP.md](../ROADMAP.md).

## FAQ

**Mitts remplace-t-il Cargo ?**
Mitts lit les manifestes et fichiers de verrouillage Cargo existants et fournit le flux de travail de
build/test Rust pour ce projet. Ses propres flux de travail CI et de release utilisent Mitts
sans invoquer Cargo. Cargo reste à la disposition des utilisateurs externes qui en ont besoin.

**Puis-je l'utiliser dans l'intégration continue (CI) ?**
Oui. La CI de ce dépôt installe la dernière version stable de Mitts, puis l'utilise pour
les builds natifs, les tests, la couverture de code, le formatage et Clippy. Le magasin d'artefacts partagé
est local à chaque exécuteur ; un cache distant multi-machines n'est pas encore disponible.

**Entrera-t-il en conflit avec rust-analyzer ou les sorties existantes de Cargo ?**
Mitts utilise les mêmes chemins de sortie par défaut que Cargo. Son magasin partagé d'artefacts reste
séparé, et `mitts clean` ne supprime que les sorties indexées de Mitts qui correspondent encore
à leur contenu enregistré ; il préserve les fichiers Cargo non liés ou remplacés.
Si Cargo détient le verrou du répertoire de build d'un profil, le nettoyage est ignoré tant que ce
build n'est pas terminé.

**Où se trouve le cache et comment le vider ?**
`--cache-dir`, sinon `$MITTS_CACHE_DIR`, `$XDG_CACHE_HOME/mitts` ou
`~/.cache/mitts` ; tout ce que Mitts met en cache réside sous cette racine unique. Utilisez
`mitts gc` pour nettoyer dans la limite des quotas ; supprimer le répertoire est également sans danger lorsqu'aucun
build n'est en cours.

**Pourquoi une unité n'a-t-elle pas été mise en cache ?**
Exécutez `mitts explain`. La commande détaille chaque unité et la raison pour laquelle elle est ou non
éligible au cache.

## Règles de conception

- Garder l'orchestration du build et du cache indépendante du langage. La sémantique du langage réside
  dans les adaptateurs ; Rust est le premier.
- Un succès de cache nécessite des entrées complètes et suivies. Les entrées inconnues sont traitées comme
  modifiées.
- Les artefacts partagés sont immuables et vérifiés avant utilisation. L'état mutable reste
  dans chaque espace de travail. Pas de liens matériels vers des fichiers modifiables, pas d'astuces sur les horodatages.
- Le nettoyage ne touche jamais aux artefacts d'un build en cours.

Règles pour les contributeurs et agents : [AGENTS.md](../AGENTS.md).
Décisions d'architecture : [docs/decisions](../docs/decisions).

## Auteur

[Alexander Panasenko](https://prod.codes/about/) · alex@prod.codes ·
[mittsbuild.com](https://mittsbuild.com)

## Citation

Pour citer Mitts, utilisez la version archivée. Le DOI de concept [10.5281/zenodo.24030067](https://doi.org/10.5281/zenodo.24030067) pointe toujours vers la dernière version ; le DOI de la version 0.3.32 est [10.5281/zenodo.24100051](https://doi.org/10.5281/zenodo.24100051). La fonction « Cite this repository » de GitHub lit [CITATION.cff](../CITATION.cff).

```bibtex
@software{panasenko_2026_24100051,
  author    = {Panasenko, Alexander},
  title     = {Mitts Build — Rust-first build system with a machine-wide, content-addressed artifact cache},
  year      = {2026},
  publisher = {Zenodo},
  version   = {0.3.32},
  doi       = {10.5281/zenodo.24100051},
  url       = {https://doi.org/10.5281/zenodo.24100051}
}
```

## Licence

Sous double licence [MIT](../LICENSE-MIT) OU [Apache-2.0](../LICENSE-APACHE), selon votre choix.
