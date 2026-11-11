<!-- translation-source: 1a926080b49fae712d44fdae44fb6f96b80d3499 -->
> [!NOTE]
> Esta é uma tradução da documentação oficial em inglês no commit [1a92608](https://github.com/alex09x/mitts/blob/1a926080b49fae712d44fdae44fb6f96b80d3499/README.md). A versão canônica e mais atualizada é o [README em inglês](https://github.com/alex09x/mitts/blob/main/README.md).

<p align="center">
  <img src="../site/logo.svg" width="96" height="96" alt="Mitts Build logo">
</p>

<h1 align="center">Mitts Build</h1>

<p align="center">
  <b>Trabalho de compilação, contabilizado.</b><br>
  Um sistema de build focado em Rust com cache de artefatos endereçado por conteúdo para toda a máquina
  e rastreamento explícito de entradas. Ele lê seus arquivos <code>Cargo.toml</code> e <code>Cargo.lock</code> existentes.
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
  <a href="#quick-start">Início rápido</a> ·
  <a href="#how-it-works">Como funciona</a> ·
  <a href="#architecture">Arquitetura</a> ·
  <a href="../docs/architecture.md">Protocolo</a> ·
  <a href="#limitations">Limitações</a> ·
  <a href="../ROADMAP.md">Roteiro</a>
</p>

<p align="center">
  <a href="https://github.com/alex09x/mitts/blob/main/README.md">English</a> ·
  <a href="https://github.com/alex09x/mitts/blob/main/readme/README.ru.md">Русский</a> ·
  <a href="https://github.com/alex09x/mitts/blob/main/readme/README.zh-CN.md">简体中文</a> ·
  <a href="https://github.com/alex09x/mitts/blob/main/readme/README.es.md">Español</a> ·
  <a href="https://github.com/alex09x/mitts/blob/main/readme/README.pt-BR.md"><b>Português (Brasil)</b></a> ·
  <a href="https://github.com/alex09x/mitts/blob/main/readme/README.ja.md">日本語</a> ·
  <a href="https://github.com/alex09x/mitts/blob/main/readme/README.de.md">Deutsch</a> ·
  <a href="https://github.com/alex09x/mitts/blob/main/readme/README.fr.md">Français</a> ·
  <a href="https://github.com/alex09x/mitts/blob/main/readme/README.ko.md">한국어</a> ·
  <a href="https://github.com/alex09x/mitts/blob/main/readme/README.ar.md">العربية</a>
</p>

---

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="../docs/img/bench-hero-dark.svg">
    <source media="(prefers-color-scheme: light)" srcset="../docs/img/bench-hero-light.svg">
    <img src="../docs/img/bench-hero-light.svg" width="920" alt="Compilar 12 worktrees de 100 projetos Rust em um Mac leva 13 minutos com o Mitts e 76 minutos com o Cargo; as execuções do compilador caem de 90,961 para 6,905 e a escrita em disco de 641 GB para 112 GB">
  </picture>
</p>

<p align="center">
  <sub>100 projetos de código aberto (90 no Linux x86_64), 12 worktrees cada, compilados ao mesmo tempo em três máquinas. Cargo e Cargo + sccache como linhas de base, incluindo os casos em que o Mitts não é mais rápido: <a href="#performance">Desempenho</a> · <a href="https://mittsbuild.com/the-hundred/">The Hundred</a></sub>
</p>

O Mitts planeja o workspace por conta própria e invoca o `rustc` diretamente. Cada
ação de compilação recebe uma chave derivada de todas as suas entradas. A saída é armazenada
uma única vez em um armazenamento imutável em toda a máquina e restaurada em cada workspace que
precisar dela, entre projetos, worktrees e branches.

Se uma entrada não puder ser contabilizada, a ação é reconstruída, não adivinhada.

## O problema

O cache de build do Cargo fica no diretório `target/` de cada workspace. Qualquer
coisa que crie um novo `target/` começa do zero:

- um segundo checkout ou `git worktree` do mesmo repositório,
- alternar para uma branch que altere o `Cargo.lock`, e depois voltar,
- outro projeto que depende dos mesmos crates com as mesmas features,
- agentes de codificação paralelos, cada um trabalhando em sua própria worktree.

O mesmo `serde`, `syn` ou `tokio` é compilado repetidamente, e cada cópia
consome espaço em disco.

As soluções alternativas comuns têm arestas afiadas:

- **Um `CARGO_TARGET_DIR` compartilhado** serializa as compilações sob um único lock, e
  projetos com conjuntos de features diferentes sobrescrevem os artefatos uns dos outros.
- **Um wrapper de compilador como o sccache** enxerga uma chamada do `rustc` por vez. Sua
  chave depende de caminhos absolutos a menos que sejam normalizados, e crates que
  invocam o linker (`bin`, `proc-macro`, `dylib`) não são cacheados. O Cargo ainda
  planeja a compilação e executa todos os scripts de build.

O Mitts segue um caminho diferente: ele substitui o executor do Cargo, não o compilador.
Ele enxerga todo o grafo de unidades, de modo que pode derivar a chave de cada unidade pelas chaves
de suas dependências, cachear unidades de linkagem e execuções de scripts de build, e permitir
que várias compilações compartilhem um único armazenamento com segurança.

## Destaques

- **Reutilização entre workspaces.** Um armazenamento por máquina. Uma dependência compilada para
  um checkout é restaurada no próximo com as mesmas entradas.
- **Chaves completas.** Toolchain, target, perfil, flags, features, conteúdo do
  código-fonte, chaves de dependências e ambiente rastreado. Entradas desconhecidas implicam
  reconstrução.
- **Manifestos do Cargo continuam sendo o contrato.** Nenhum manifesto novo, nenhum segundo lockfile.
  Os fluxos de build, teste, lint, cobertura e código-fonte de lançamento deste repositório
  são executados pelo Mitts; eles não invocam a CLI do Cargo. A interoperabilidade externa
  com o Cargo permanece disponível para os consumidores.
- **Unidades de linkagem e scripts de build são cacheados.** Binários, testes, proc macros
  e execuções de scripts de build recebem chave e são armazenados, não apenas `rlib`s.
- **Compilações concorrentes cooperam.** Locks por chave: dois terminais ou worktrees
  que precisem da mesma dependência a compilam uma única vez. Um jobserver limita
  o paralelismo para todos os processos do Mitts na máquina.
- **Compartilhamento seguro.** Os objetos no armazenamento são imutáveis: publicados atomicamente,
  armazenados como somente leitura e restaurados via reflink ou cópia. `MITTS_VERIFY_RESTORE=1` recalcula o hash de cada
  objeto antes de restaurá-lo (o padrão confia no armazenamento, o que economiza uma leitura de
  cada byte restaurado). Onde o sistema de arquivos não suporta reflink, a restauração cria um hardlink
  do objeto somente leitura do armazenamento no `target/` em vez de copiar seus bytes: o inode compartilhado
  permanece `0o444` (`0o555` para executáveis), o índice do target rejeita links que ganharam
  permissões de escrita, e o Mitts só substitui arquivos de workspace por renomeação. Nunca há um link
  com permissão de escrita para um objeto do armazenamento. `MITTS_STORE_HARDLINKS=0` mantém cópias privadas para workspaces
  onde outras ferramentas escrevem diretamente no local.
- **Staging de workspace sem cópia.** Aliases de bibliotecas dentro do workspace, binários secundários
  e staging atômico de unidades usam hardlinks internos do workspace e promoção por renomeação sem
  copiar bytes. Quando o `MITTS_CACHE_DIR` e o `target/` do workspace residem no mesmo
  volume de sistema de arquivos, as promoções de staging operam sem copiar bytes; entre pontos
  de montagem separados, eles recorrem de forma transparente a cópias seguras.
- **Um caminho rápido sem operação (no-op).** Um workspace inalterado pula o planejamento e a geração de chaves.
- **Uma única CLI.** `build`, `run`, `check`, `clippy`, `fmt`, `test` (com doctests),
  `coverage`, `explain`, `fetch`, `gc`, `clean`.

<a id="quick-start"></a>
## Início rápido

Instale a versão nativa assinada mais recente:

```sh
curl -fsSL https://mittsbuild.com/install.sh | sh
```

Para compilar a partir do checkout do código-fonte, primeiro instale a versão mais recente lançada do Mitts e, em seguida,
use o Mitts para compilar o checkout:

```sh
git clone https://github.com/alex09x/mitts
cd mitts
mitts build --release
MITTS_JOBS=2 mitts build     # limit concurrent native build units
install -m755 target/release/mitts "$HOME/.cargo/bin/mitts"
```

Em seguida, execute-o dentro de qualquer workspace do Cargo:

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

Para projetos Git, o empacotamento atualmente requer um repositório limpo com o pacote em sua raiz.
Padrões `include` do manifesto podem selecionar arquivos ignorados; pacotes de workspace aninhados
e diretórios com links simbólicos ainda não são suportados. O `mitts publish` tem como alvo o crates.io e lê
o token de `CARGO_REGISTRY_TOKEN`.

O Mitts grava as saídas de compilação nos diretórios usuais do Cargo `target/debug/` e `target/release/`.
`CARGO_TARGET_DIR` e `--target-dir` selecionam uma raiz de saída diferente.
Seu cache compartilhado é separado
(`--cache-dir`, caso contrário `$MITTS_CACHE_DIR`, caso contrário `$XDG_CACHE_HOME/mitts`, caso contrário
`~/.cache/mitts`). O armazenamento de artefatos, o cache de ações, os locks, o jobserver, o armazenamento
de fontes, os espelhos git e o cache de índice residem todos sob essa mesma raiz.

Dependências Git, públicas ou privadas, são clonadas pelo próprio Mitts com o
`git` do sistema, de modo que suas chaves SSH, agente, auxiliares de credenciais e `GIT_SSH_COMMAND` se aplicam
exatamente como na linha de comando; prompts interativos são desativados, portanto uma credencial ausente
falha em vez de travar. Nada passa pelo Cargo. Um checkout que o Cargo já
tenha feito em `~/.cargo/git/checkouts` é reutilizado em vez de clonado novamente.

Todas as outras formas que o Cargo utiliza para obter um crate funcionam da mesma maneira, a partir da mesma configuração:

- **Registros alternativos**: `[registries.<name>] index = "sparse+https://…"`
  e `registry = "<name>"` na dependência. Os tokens vêm dos provedores de credenciais
  do Cargo, na ordem do Cargo: `cargo:token` (o ambiente e
  `credentials.toml`), `cargo:macos-keychain`, `cargo:libsecret`,
  `cargo:token-from-stdout <command>`, e plugins como
  `cargo-credential-1password` por meio do protocolo de provedor de credenciais,
  configurados por `[registry] global-credential-providers`,
  `[registries.<name>] credential-provider` e `[credential-alias]`. Um registro
  cujo `config.json` declare `auth-required` falha com uma mensagem clara quando nenhum
  provedor possuir um token. Registros baseados em índice Git são recusados: use o protocolo
  sparse.
- **Espelhos**: `[source.crates-io] replace-with = "<name>"` com
  `[source.<name>] registry = "sparse+https://…"`. Os bytes vêm do espelho,
  o `Cargo.lock` continua nomeando crates.io, exatamente como o Cargo grava.
- **Fontes vendored**: `[source.vendored-sources] directory = "vendor"` a partir de
  `cargo vendor`. Os pacotes são verificados contra seu `.cargo-checksum.json`
  e copiados para o armazenamento; nada acessa a rede, pacotes
  git inclusos. Sem um lockfile, o diretório de vendor serve como índice.
- **`[patch]`**: `[patch.crates-io] foo = { path = … }` ou `{ git = … }`
  substitui o pacote do registro; sem um lockfile a substituição é
  resolvida e fixada, devendo satisfazer o requisito, assim como no Cargo.

`[env]` em uma configuração do Cargo é aplicado ao rustc, scripts de build e aos programas
que `mitts run` e `mitts test` iniciam, com as regras de `force` e `relative` do Cargo;
os arquivos de configuração são entradas, de modo que um valor alterado recompila o que o leu.

Sem um `Cargo.lock`, o Mitts resolve um da mesma forma que o Cargo: versão
mais alta não retirada (non-yanked) por requisito, features fluindo pelas arestas, uma
dependência opcional incluída apenas quando uma feature a ativa, uma versão por
faixa de compatibilidade. O resultado é um lockfile que o Cargo aceita como seu.

<a id="how-it-works"></a>
## Como funciona

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

A chave de uma unidade inclui as **chaves** de suas dependências, não seus bytes de saída, portanto uma
alteração em qualquer lugar abaixo de uma unidade altera sua chave. Scripts de build são executados em um diretório
de staging; suas diretivas (`rustc-cfg`, `rustc-link-*`, `rustc-env`, …) entram
na chave da unidade que configuram, e suas declarações `rerun-if-*` decidem
quando uma execução em cache pode ser reutilizada.

Após uma compilação bem-sucedida, o Mitts registra um instantâneo do workspace. Se nada
nele mudou, a próxima compilação salta direto para o resultado. Qualquer incompatibilidade recorre
à compilação completa com geração de chaves, de forma que o caminho rápido nunca ignora o trabalho que o protocolo
faria.

O protocolo completo — derivação de chaves, locks, scripts de build, o caminho rápido,
layout do armazenamento e coleta de lixo — está em
[docs/architecture.md](../docs/architecture.md).

<a id="architecture"></a>
## Arquitetura

O Mitts é um núcleo neutro em relação a linguagens com adaptadores de linguagem por cima. O núcleo não sabe
nada sobre o Cargo; o adaptador Rust transforma um workspace do Cargo em ações
com chaves.

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

| Caminho | O que vive lá |
| --- | --- |
| `src/cli/` | Manipuladores de comandos: `build`, `test`, `clippy`, `explain`, … |
| `src/adapter/rust/` | Semântica do Cargo: manifestos, lockfile, índice sparse, resolução de features, planejamento de unidades, chaves de ação, executor de DAG, scripts de build, doctests |
| `src/adapter/c/` | Adaptador experimental de C/C++ no mesmo armazenamento |
| `src/core/` | Armazenamento independente de linguagem (`cas.rs`), cache de ações, locks, jobserver, caminho rápido (`build_lock*.rs`), GC, reflinks |
| `docs/` | [Protocolo](../docs/architecture.md), [decisões de design](../docs/decisions), [cenários](../docs/scenarios) |
| `tests/` | Testes de integração, incluindo paridade com Cargo e testes de invalidação de cache |
| `scripts/` | Harness de benchmark e auditoria |
| `fixtures/` | Projetos mínimos de Rust e C usados pelos testes |

## O que invalida o cache

Um acerto de cache requer que todas as entradas abaixo correspondam. Se o Mitts não puder ver uma
entrada, ele não reutiliza o resultado.

| Entrada | Rastreado como |
| --- | --- |
| Toolchain | saída de `rustc -vV` |
| Target e perfil | tripla do target, configurações de perfil, `RUSTFLAGS` e rustflags da configuração do Cargo |
| Features | conjunto de features resolvidas da unidade (resolver 1 ou 2, assim como o Cargo faria) |
| Fontes | hashes de conteúdo dos arquivos do pacote e de cada arquivo que o rustc informa ler (`include_str!`, `include_bytes!`, módulos) |
| Dependências | as chaves de ação de todas as unidades de dependência |
| Ambiente | Configurações relevantes de `CARGO_*` e `RUST_*`, e variáveis lidas via `env!` / `option_env!`, com `<unset>` gravado explicitamente |
| Configuração do Cargo | os arquivos `.cargo/config(.toml)` que o Cargo leria |
| Scripts de build | suas diretivas, `OUT_DIR`, e o conteúdo dos caminhos de `rerun-if-changed` e valores das variáveis de `rerun-if-env-changed` |

Programas configurados como `rustc-wrapper` e `rustc-workspace-wrapper` são invocados para
compilação. Como um wrapper arbitrário pode depender de entradas não rastreadas, o Mitts
desativa a reutilização do cache de ações e do cache de scripts de build enquanto qualquer um dos wrappers estiver ativo.
Isso pode tornar essas compilações mais lentas.

Algumas proc macros leem arquivos do seu pacote durante sua expansão: `sqlx` verifica
consultas em relação a `.sqlx/`, `askama` compila `templates/`, `rust-embed` e
`include_dir` incorporam diretórios inteiros. O rustc só toma conhecimento dessas leituras quando
a macro as registra; portanto, declare esses crates no `Cargo.toml` raiz:

```toml
[workspace.metadata.mitts]          # [package.metadata.mitts] in a single crate
impure-proc-macros = ["sqlx", "askama", "rust-embed"]
```

Crates que dependem de uma macro listada recebem chave com base em cada arquivo do seu pacote
e no seu caminho absoluto: um template editado força sua recompilação, e outro
checkout nunca os reutiliza. Todas as outras macros são tratadas como puras, e seus
dependentes são compartilhados entre worktrees. Nomeie o crate do qual você depende; o
crate de macro que ele puxa (`sqlx-macros`) também funciona.

## Comparação

Uma comparação qualitativa de como cada ferramenta reutiliza o trabalho. Não é uma
afirmação de desempenho.

| | Cargo | Cargo + sccache | Mitts |
| --- | --- | --- | --- |
| Lê `Cargo.toml` / `Cargo.lock` no estado em que estão | sim | sim | sim |
| Reutilização entre workspaces e worktrees | não (por `target/`) | sim, por chamada do `rustc` | sim, por unidade |
| A chave depende do caminho do checkout | — | sim, a menos que normalizado | não para crates do registro; para crates de workspace apenas quando não elegíveis |
| Cacheia unidades `bin`, de teste e `proc-macro` | — | não | sim |
| Cacheia execuções de scripts de build | não | não | sim |
| Compilações concorrentes de uma dependência | aguardam no lock de `target/` | ambas podem compilar quando não há resultado no cache | uma compila, a outra reutiliza |
| Cache remoto / de equipe | não | sim (S3, GCS, Redis, …) | planejado |
| Windows | sim | sim | não |

Bazel e Buck2 resolvem um problema maior — compilações herméticas e multilíngues com
execução remota —, mas exigem seus próprios arquivos de build em vez do `Cargo.toml`.

<a id="limitations"></a>
## Limitações

O Mitts está em pré-lançamento. Conheça estes pontos antes de confiar nele:

- **Apenas Linux e macOS.** Locks e o jobserver são POSIX.
- **Apenas target host.** Compilação cruzada ainda não é suportada.
- **Sem cache remoto ainda.** O armazenamento é por máquina.
- **Apenas registros sparse.** Um registro servido como índice git (`registry+…`
  diferente de crates.io, ou `[source] registry = "https://…"` sem
  `sparse+`) e fontes `local-registry` são recusados com orientação.
- **Credenciais são lidas, nunca gravadas.** `cargo login` continua sendo tarefa do Cargo;
  `cargo:wincred` é exclusivo para Windows e é ignorado.
- **Algumas unidades nunca são cacheadas:** crates do tipo `cdylib`, `dylib` ou
  `staticlib`, e crates cujas leituras de `env!` não podem ser rastreadas. Elas sempre
  são compiladas; elas são informadas pelo `mitts explain`.
- **Crates de workspace entre checkouts.** Seus próprios crates são reutilizados entre
  worktrees apenas quando sua chave puder se tornar independente de caminho (sem
  leituras de `CARGO_MANIFEST_DIR`, sem script de build, sem proc macro impura declarada).
  Caso contrário, a chave inclui o caminho absoluto. No macOS, as informações de depuração (debug stabs)
  do linker são gravadas relativas ao workspace (`-oso_prefix`), de modo que binários linkados
  também são compartilhados; inicie o depurador na raiz do workspace para que ele
  encontre os arquivos de objeto.
- **Macros que leem seus arquivos cabem a você declarar.** Uma proc macro é tratada
  como pura a menos que o projeto a liste (veja acima). Os dependentes de uma macro listada
  acompanham cada arquivo em seu pacote; uma macro não listada que lê um arquivo
  sem registrá-lo no rustc pode produzir um resultado desatualizado, exatamente como
  no Cargo.
- **Scripts de build não estão em sandbox.** Um script que lê arquivos ou variáveis que
  não declarou com `rerun-if-*` pode produzir um resultado desatualizado — exatamente como no
  Cargo.
- **Recursos não suportados do Cargo falham ruidosamente.** Por exemplo `--profile`, globs
  com curingas em diretórios intermediários em `workspace.members`. O Mitts
  os relata em vez de adivinhar.
- **Zero-copy no mesmo sistema de arquivos.** A promoção de unidade sem cópia a partir do staging requer
  que o `MITTS_CACHE_DIR` e o diretório `target/` do workspace residam no mesmo
  ponto de montagem de sistema de arquivos. Ultrapassar os limites do sistema de arquivos recorre de forma segura à cópia padrão.

**Não use o Mitts ainda** se você precisar de Windows, compilação cruzada, cache de equipe ou
uma versão estável. O Cargo, opcionalmente com sccache, é a escolha segura nesses casos.

<a id="performance"></a>
## Desempenho

Medido com o Mitts `0.3.17` em **100 projetos Rust de código aberto** no macOS (Apple M2 Ultra, APFS) e Linux ARM64 (XFS), e **90** no Linux x86_64 (AMD Ryzen 9 9950X, XFS), contra o `cargo` puro e `cargo` com `sccache` (`0.17.0`). Cada leitura é a mediana de 3 repetições com verificação de paridade de diagnósticos, códigos de saída e resultados de testes; todos os projetos medidos permanecem nos totais (`0`, `7` e `0` execuções sinalizadas com ruído), e nenhum número excede a medição: as linhas de base são truncadas e o Mitts é arredondado para cima ([ROADMAP M8](../ROADMAP.md)). O estudo completo, cada projeto e o método estão em [mittsbuild.com/the-hundred](https://mittsbuild.com/the-hundred/).

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="../docs/img/bench-scenarios-dark.svg">
    <source media="(prefers-color-scheme: light)" srcset="../docs/img/bench-scenarios-light.svg">
    <img src="../docs/img/bench-scenarios-light.svg" width="920" alt="Tempo de compilação do projeto mediano em seis cenários: Cargo, Cargo + sccache e Mitts">
  </picture>
</p>

Cada linha mostra o tempo de compilação do projeto mediano (a própria mediana de cada ferramenta sobre os projetos, escala linear por linha), com o número de projetos em que o Mitts terminou primeiro ao lado. No macOS, o projeto mediano compila suas 12 worktrees em **32 s** com `cargo`, **31 s** com `cargo` + `sccache` e **3.8 s** com o Mitts. Com o armazenamento local aquecido, uma segunda worktree limpa leva **1.7 s**, **1.6 s** e **79 ms**; uma troca de branch **627 ms**, **209 ms** e **27 ms**; e uma compilação sem alterações **100 ms**, **41 ms** e **9 ms**. O Mitts termina primeiro em 99–100 de 100 projetos nesses quatro cenários (71 de 71 na segunda worktree).

Onde o Mitts não vence:

- **Compilação a frio, cache vazio.** O `cargo` puro termina primeiro em 71 de 100 projetos no macOS (60 de 90 no Linux x86_64, 77 de 100 no Linux aarch64), por até 6.2 s nos maiores, porque o Mitts calcula o hash das entradas e preenche um armazenamento vazio. O projeto mediano leva **1.7 s** com `cargo` e **2.0 s** com o Mitts. O Mitts fica à frente de um `sccache` a frio (**2.4 s**, Mitts primeiro em 86 de 100).
- **Editar um arquivo.** O projeto mediano leva **364 ms** com `cargo`, **596 ms** com `sccache` e **315 ms** com o Mitts, e o Mitts termina primeiro em 49 de 71 projetos contra `cargo`. O `cargo` termina primeiro nos outros 22 (21 de 64 no x86_64, 29 de 71 no aarch64), por até 2.6 s no macOS e 8.6 s no x86_64, porque reutiliza a compilação incremental do `rustc` dentro de um crate grande enquanto o Mitts armazena unidades inteiras de crate em cache.
- **Repetição de 10 commits** (fora do gráfico): resultado dividido, com o Mitts primeiro em 58 de 100 projetos no macOS e medianas próximas (**5.6 s** com `cargo`, **5.8 s** com o Mitts).

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="../docs/img/bench-swarm-dark.svg">
    <source media="(prefers-color-scheme: light)" srcset="../docs/img/bench-swarm-light.svg">
    <img src="../docs/img/bench-swarm-light.svg" width="920" alt="Placar de compilação de 12 worktrees em 100 projetos Rust: tempo, CPU, execuções do compilador e escrita em disco">
  </picture>
</p>

Compilando **12 worktrees paralelas** em 4 commits em todos os projetos, totais por máquina:

| Máquina | Tempo: `cargo` / `sccache` / Mitts | Execuções do `rustc`: `cargo` / Mitts | Escrita em disco: `cargo` / Mitts |
|---|---|---|---|
| macOS, APFS, M2 Ultra (100 projetos) | 76 min / 73 min / **13 min** | 90,961 / **6,905** | 641 GB / **112 GB** |
| Linux x86_64, XFS, Ryzen 9 9950X (90 projetos) | 60 min / 52 min / **9 min** | 75,049 / **6,162** | 651 GB / **106 GB** |
| Linux aarch64, XFS (100 projetos) | 78 min / 76 min / **22 min** | 90,544 / **7,348** | 743 GB / **121 GB** |

O tempo de CPU no macOS cai de **20.3 h** (`cargo`) e **8.0 h** (`sccache`) para **2.7 h**. A máquina x86_64 mediu 90 projetos, não 100: dez projetos estão ausentes dessa execução (similar, smallvec, smol, syntect, tealdeer, tera, textwrap, tokei, tower-http, xh), e a causa não foi registrada.


## Status

| Área | Estado |
| --- | --- |
| Leitura de `Cargo.toml` / `Cargo.lock`, resolução de índice sparse | concluído |
| Resolução de features (resolver 1 e 2) | concluído |
| Fetcher paralelo e armazenamento global de fontes | concluído |
| Executor de DAG nativo, `rustc` pipelined, jobserver | concluído |
| Armazenamento endereçado por conteúdo e cache de ações | concluído |
| Execução e cache de scripts de build | concluído |
| Caminho rápido no-op | concluído |
| `check`, `clippy`, `fmt`, `test` (incl. doctests), `coverage` | concluído |
| GC conservador com cotas | concluído |
| Adaptador C/C++ no mesmo armazenamento | experimental |
| Chaves independentes de caminho para crates do workspace | parcial |
| Estudo de benchmark reproduzível | concluído |
| Cache remoto / de equipe | planejado |
| Windows, compilação cruzada | não suportado |

Marcos e evidências: [ROADMAP.md](../ROADMAP.md).

## FAQ

**O Mitts substitui o Cargo?**
O Mitts lê os manifestos e lockfiles existentes do Cargo e fornece o fluxo de trabalho
de build/teste em Rust para este projeto. Seus próprios fluxos de CI e release usam o Mitts
sem invocar o Cargo. O Cargo permanece disponível para usuários externos que precisam dele.

**Posso usá-lo em CI?**
Sim. O CI deste repositório instala o Mitts estável mais recente e o utiliza para
compilações nativas, testes, cobertura, formatação e Clippy. O armazenamento compartilhado de artefatos
é local para cada executor; um cache remoto entre máquinas ainda não está disponível.

**Haverá conflito com o rust-analyzer ou saídas existentes do Cargo?**
O Mitts usa os mesmos caminhos de saída padrão do Cargo. Seu armazenamento compartilhado de artefatos permanece
separado, e `mitts clean` remove apenas as saídas indexadas do Mitts que ainda correspondem
ao conteúdo registrado; ele preserva arquivos do Cargo não relacionados ou substituídos.
Se o Cargo mantiver o lock do diretório de build de um perfil, a limpeza será ignorada até que essa
compilação termine.

**Onde fica o cache e como posso limpá-lo?**
`--cache-dir`, caso contrário `$MITTS_CACHE_DIR`, `$XDG_CACHE_HOME/mitts` ou
`~/.cache/mitts`; tudo o que o Mitts armazena em cache fica sob essa raiz única. Use
`mitts gc` para podar dentro das cotas; excluir o diretório também é seguro quando nenhuma
compilação estiver em execução.

**Por que uma unidade não foi cacheada?**
Execute `mitts explain`. Ele lista cada unidade e a razão pela qual ela é ou não
elegível para o cache.

## Regras de design

- Mantenha a orquestração de build e cache neutra em relação a linguagens. A semântica de linguagem vive
  em adaptadores; Rust é o primeiro deles.
- Um acerto de cache precisa de entradas completas e rastreadas. Entradas desconhecidas são tratadas como
  alteradas.
- Artefatos compartilhados são imutáveis e verificados antes do uso. O estado mutável permanece
  em cada workspace. Sem hardlinks para arquivos graváveis, sem truques com timestamps.
- A limpeza nunca toca em artefatos de uma compilação ativa.

Regras para colaboradores e agentes: [AGENTS.md](../AGENTS.md).
Decisões de arquitetura: [docs/decisions](../docs/decisions).

## Autor

[Alexander Panasenko](https://prod.codes/about/) · alex@prod.codes ·
[mittsbuild.com](https://mittsbuild.com)

## Como citar

Para citar o Mitts, use a versão arquivada. O DOI de conceito [10.5281/zenodo.24030067](https://doi.org/10.5281/zenodo.24030067) sempre aponta para a versão mais recente; o DOI da versão 0.3.34 é [10.5281/zenodo.24125493](https://doi.org/10.5281/zenodo.24125493). A opção «Cite this repository» do GitHub lê o [CITATION.cff](../CITATION.cff).

```bibtex
@software{panasenko_2026_24125493,
  author    = {Panasenko, Alexander},
  title     = {Mitts Build — Rust-first build system with a machine-wide, content-addressed artifact cache},
  year      = {2026},
  publisher = {Zenodo},
  version   = {0.3.34},
  doi       = {10.5281/zenodo.24125493},
  url       = {https://doi.org/10.5281/zenodo.24125493}
}
```

## Licença

Duplamente licenciado sob [MIT](../LICENSE-MIT) OU [Apache-2.0](../LICENSE-APACHE), à sua escolha.
