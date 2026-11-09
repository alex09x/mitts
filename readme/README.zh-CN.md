<!-- translation-source: 1a926080b49fae712d44fdae44fb6f96b80d3499 -->

> [!NOTE]
> 本文是官方英文文档在提交 [1a92608](https://github.com/alex09x/mitts/blob/1a926080b49fae712d44fdae44fb6f96b80d3499/README.md) 处的简体中文翻译。权威且最新的版本请参阅[英文 README](https://github.com/alex09x/mitts/blob/main/README.md)。

<p align="center">
  <img src="../site/logo.svg" width="96" height="96" alt="Mitts Build logo">
</p>

<h1 align="center">Mitts Build</h1>

<p align="center">
  <b>构建工作，悉数记录。</b><br>
  一款 Rust 优先的构建系统，具备机器级内容寻址工件缓存与显式输入追踪功能。
  它直接读取现有的 <code>Cargo.toml</code> 和 <code>Cargo.lock</code>。
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
  <a href="#quick-start">快速开始</a> ·
  <a href="#how-it-works">工作原理</a> ·
  <a href="#architecture">架构</a> ·
  <a href="../docs/architecture.md">协议</a> ·
  <a href="#limitations">局限性</a> ·
  <a href="../ROADMAP.md">路线图</a>
</p>

<p align="center">
  <a href="https://github.com/alex09x/mitts/blob/main/README.md">English</a> ·
  <a href="https://github.com/alex09x/mitts/blob/main/readme/README.ru.md">Русский</a> ·
  <a href="https://github.com/alex09x/mitts/blob/main/readme/README.zh-CN.md"><b>简体中文</b></a> ·
  <a href="https://github.com/alex09x/mitts/blob/main/readme/README.es.md">Español</a> ·
  <a href="https://github.com/alex09x/mitts/blob/main/readme/README.pt-BR.md">Português (Brasil)</a> ·
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
    <img src="../docs/img/bench-hero-light.svg" width="920" alt="在 Mac 上同时构建 100 个 Rust 项目的 12 个 worktree，Mitts 耗时 13 分钟而 Cargo 耗时 76 分钟；编译器调用次数从 90,961 次降至 6,905 次，磁盘写入量从 641 GB 降至 112 GB">
  </picture>
</p>

<p align="center">
  <sub>100 个开源项目（Linux x86_64 上为 90 个），每个项目 12 个 worktree 同时构建，在三台机器上实测。以 Cargo 和 Cargo + sccache 为基线，包含 Mitts 未占优的场景：<a href="#performance">性能</a> · <a href="https://mittsbuild.com/the-hundred/">The Hundred</a></sub>
</p>

Mitts 自行规划工作区并直接调用 `rustc`。每个编译动作都会获得一个派生自其全部输入的键（key）。输出仅在机器范围内的不可变存储中保存一次，并复原到需要它的每个工作区中，跨越不同项目、worktree 与分支。

如果某个输入无法被核算追踪，该动作将被重新构建，而非凭空猜测。

## 问题所在

Cargo 的构建缓存位于每个工作区的 `target/` 目录中。任何创建新 `target/` 的操作都会从零开始：

- 同一代码仓库的第二份检出或 `git worktree`，
- 切换到修改了 `Cargo.lock` 的分支，然后再切回来，
- 依赖于相同 crate 及相同特性的另一个项目，
- 各自在独立 worktree 中工作的并行编码 Agent。

同一个 `serde`、`syn` 或 `tokio` 会被反复编译，并且每份副本都会占用磁盘空间。

常见的通融方案都有明显的弊端：

- **共享 `CARGO_TARGET_DIR`** 会在单一锁背后将构建串行化，且特性集不同的项目会相互覆盖彼此的工件。
- **类似 sccache 的编译器封装器** 每次只看到单个 `rustc` 调用。除非进行标准化，否则其缓存键取决于绝对路径；此外调用链接器的 crate（`bin`、`proc-macro`、`dylib`）均不予缓存。Cargo 依然需要规划构建并运行每一个 build script。

Mitts 采用了不同的路线：它替换的是 Cargo 的执行器，而不是编译器。它能洞察整个单元图（unit graph），因此可以根据各单元依赖项的键来为每个单元计算键值，缓存链接单元与 build script 的运行，并让多个构建安全共享同一个存储。

## 亮点特性

- **跨工作区复用。** 每台机器一个存储。为一个检出编译好的依赖项，会在输入相同时直接复原到下一个检出中。
- **完整的键计算。** 工具链、目标架构、profile、编译标志、特性、源码内容、依赖项的键以及受追踪的环境变量。未知输入即意味着重新构建。
- **Cargo 清单始终作为契约。** 无需新清单，无需第二个 lockfile。本代码仓库的构建、测试、代码检查、覆盖率和发布源码工作流均通过 Mitts 运行；它们不调用 Cargo CLI。外部 Cargo 互操作性仍然对使用者可用。
- **链接单元与 build script 均被缓存。** 二进制文件、测试、proc macro 以及 build-script 执行均被编入键并予以存储，而不仅限于 `rlib`。
- **并发构建协同工作。** 基于键的细粒度锁：两个需要相同依赖的终端或 worktree 只会编译一次。同一个 jobserver 限制机器上所有 Mitts 进程的并行度。
- **安全共享。** 存储对象不可变：原子发布、只读存储并通过 reflink 或复制进行复原。`MITTS_VERIFY_RESTORE=1` 会在复原前对每个对象重新计算哈希（默认情况下信任存储，从而省去对每个复原字节的一次读取）。在文件系统不支持 reflink 的情况下，复原操作会将只读存储对象硬链接到 `target/` 中，而不是复制其字节：共享 inode 保持 `0o444`（可执行文件为 `0o555`），目标索引会拒绝获得写权限位的链接，且 Mitts 仅通过重命名替换工作区文件。绝不会出现指向存储对象的可写链接。`MITTS_STORE_HARDLINKS=0` 为那些会被其他工具就地写入的工作区保留私有副本。
- **零拷贝工作区暂存。** 工作区内部的库别名、辅助二进制文件以及原子单元暂存均使用工作区内硬链接和重命名提升，无需复制字节。当 `MITTS_CACHE_DIR` 与工作区 `target/` 位于同一文件系统卷上时，暂存提升无需复制字节即可完成；跨不同挂载点时，它们会透明地回退到安全复制。
- **空操作快速路径（no-op fast path）。** 未更改的工作区跳过规划与键计算。
- **统一 CLI。** `build`、`run`、`check`、`clippy`、`fmt`、`test`（含文档测试）、`coverage`、`explain`、`fetch`、`gc`、`clean`。

<a id="quick-start"></a>
## 快速开始

安装最新签名的原生发布版本：

```sh
curl -fsSL https://mittsbuild.com/install.sh | sh
```

要从源码检出构建，请先安装最新发布的 Mitts，再用 Mitts 构建该检出：

```sh
git clone https://github.com/alex09x/mitts
cd mitts
mitts build --release
MITTS_JOBS=2 mitts build     # limit concurrent native build units
install -m755 target/release/mitts "$HOME/.cargo/bin/mitts"
```

然后在任意 Cargo 工作区内运行它：

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

对于 Git 项目，打包目前要求代码仓库整洁且 package 位于其根目录。清单的 `include` 模式可以选择被忽略的文件；嵌套的工作区 package 以及符号链接目录目前尚不支持。`mitts publish` 面向 crates.io 并从 `CARGO_REGISTRY_TOKEN` 读取令牌。

Mitts 将构建输出写入 Cargo 常规的 `target/debug/` 和 `target/release/` 目录。`CARGO_TARGET_DIR` 和 `--target-dir` 可用于选择不同的输出根路径。其共享缓存是分开的（`--cache-dir`，否则为 `$MITTS_CACHE_DIR`，否则为 `$XDG_CACHE_HOME/mitts`，否则为 `~/.cache/mitts`）。工件存储、动作缓存、锁、jobserver、源码存储、git 镜像与索引缓存全部位于该单一根路径下。

无论是公开还是私有 Git 依赖，均由 Mitts 本身使用系统 `git` 进行克隆，因此您的 SSH 密钥、agent、凭据助手和 `GIT_SSH_COMMAND` 完全像在命令行上一样生效；交互提示已被禁用，因此凭据缺失会直接失败而不是挂起。没有任何操作经过 Cargo。Cargo 已经在 `~/.cargo/git/checkouts` 下检出的内容会被复用，而不会重新克隆。

Cargo 获取 crate 的所有其他方式均以相同方式运作，基于相同的配置：

- **备选注册源（Alternative registries）**：依赖项上配置 `[registries.<name>] index = "sparse+https://…"` 和 `registry = "<name>"`。令牌来自 Cargo 的凭据提供程序，按照 Cargo 的顺序：`cargo:token`（环境变量与 `credentials.toml`）、`cargo:macos-keychain`、`cargo:libsecret`、`cargo:token-from-stdout <command>`，以及通过凭据提供程序协议调用的插件（如 `cargo-credential-1password`），由 `[registry] global-credential-providers`、`[registries.<name>] credential-provider` 和 `[credential-alias]` 配置。如果某个注册源的 `config.json` 声明了 `auth-required`，且没有任何提供程序拥有令牌，则会失败并给出清晰信息。拒绝 Git 索引注册源：请使用 sparse 协议。
- **镜像源（Mirrors）**：配置 `[source.crates-io] replace-with = "<name>"` 与 `[source.<name>] registry = "sparse+https://…"`. 字节数据来自镜像，`Cargo.lock` 保持记录 crates.io，与 Cargo 写入完全一致。
- **Vendored 源码**：来自 `cargo vendor` 的 `[source.vendored-sources] directory = "vendor"`。Package 会根据其 `.cargo-checksum.json` 进行校验并复制到存储中；没有任何内容会接触网络，git package 也不例外。在没有 lockfile 的情况下，vendor 目录充当索引。
- **`[patch]`**：`[patch.crates-io] foo = { path = … }` 或 `{ git = … }` 替换注册源 package；在没有 lockfile 的情况下，替换项会被解析并固定，并且必须满足版本约束要求，这与 Cargo 一致。

Cargo 配置中的 `[env]` 会应用到 rustc、build script 以及 `mitts run` 和 `mitts test` 启动的程序中，遵循 Cargo 的 `force` 和 `relative` 规则；配置文件本身也是输入，因此修改后的值会促使读取它的部分重新构建。

在没有 `Cargo.lock` 的情况下，Mitts 会按照 Cargo 的方式进行解析：每个约束采用未撤回（non-yanked）的最高版本，特性沿边传递，可选依赖项仅在特性开启时包含，每个兼容范围对应一个版本。生成的结果是 Cargo 完全认可的 lockfile。

<a id="how-it-works"></a>
## 工作原理

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

单元的键包含其依赖项的**键**，而不是它们的输出字节，因此其依赖链中任何位置发生变动，都会改变该单元的键值。Build script 在暂存目录中运行；它们的指令（`rustc-cfg`、`rustc-link-*`、`rustc-env`、…）会进入它们所配置单元的键中，其 `rerun-if-*` 声明决定了何时可以复用缓存的运行结果。

构建成功后，Mitts 会记录工作区快照。如果其中没有任何内容变动，下一次构建将直接跳到结果。任何不匹配都会回退到全量键控构建，因此快速路径绝不会跳过协议本应执行的工作。

完整的协议——键派生、锁机制、build script、快速路径、存储布局与垃圾回收——见 [docs/architecture.md](../docs/architecture.md)。

<a id="architecture"></a>
## 架构

Mitts 是一个语言无关的核心，上层覆盖语言适配器。核心对 Cargo 一无所知；Rust 适配器负责将 Cargo 工作区转换为带键的动作。

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

| 路径 | 包含内容 |
| --- | --- |
| `src/cli/` | 命令处理程序：`build`、`test`、`clippy`、`explain`、… |
| `src/adapter/rust/` | Cargo 语义：清单、lockfile、sparse index、特性解析、单元规划、动作键、DAG 运行器、build script、文档测试 |
| `src/adapter/c/` | 基于同一存储的实验性 C/C++ 适配器 |
| `src/core/` | 语言无关的存储（`cas.rs`）、动作缓存、锁、jobserver、快速路径（`build_lock*.rs`）、GC、reflink |
| `docs/` | [协议](../docs/architecture.md)、[设计决策](../docs/decisions)、[场景说明](../docs/scenarios) |
| `tests/` | 集成测试，包括 Cargo 等价性测试与缓存失效测试 |
| `scripts/` | 基准测试与审计测试套件 |
| `fixtures/` | 测试使用的最小 Rust 和 C 项目 |

## 缓存失效条件

要命中缓存，以下每一项输入都必须匹配。如果 Mitts 无法检测到某项输入，则不会复用该结果。

| 输入 | 追踪形式 |
| --- | --- |
| 工具链 | `rustc -vV` 输出 |
| 目标架构与 profile | 目标三元组（target triple）、profile 配置、`RUSTFLAGS` 以及 Cargo 配置中的 rustflags |
| 特性 | 该单元解析后的特性集（resolver 1 或 2，与 Cargo 一致） |
| 源码 | package 文件的内容哈希以及 rustc 报告读取的每个文件（`include_str!`、`include_bytes!`、模块） |
| 依赖项 | 所有依赖单元的动作键 |
| 环境变量 | 相关的 `CARGO_*` 和 `RUST_*` 设置，以及通过 `env!` / `option_env!` 读取的变量，并显式记录 `<unset>` |
| Cargo 配置 | Cargo 会读取的 `.cargo/config(.toml)` 文件 |
| Build script | 它们的指令、`OUT_DIR`，以及 `rerun-if-changed` 路径的内容与 `rerun-if-env-changed` 变量的值 |

配置的 `rustc-wrapper` 和 `rustc-workspace-wrapper` 程序会被调用以进行编译。由于任意 wrapper 都可能依赖未经追踪的输入，因此只要任一 wrapper 处于激活状态，Mitts 就会禁用动作缓存与 build-script 缓存的复用。这可能会使这些构建变慢。

某些 proc macro 在展开期间会从 package 中读取文件：`sqlx` 针对 `.sqlx/` 校验查询，`askama` 编译 `templates/`，`rust-embed` 和 `include_dir` 嵌入整个目录。rustc 仅在宏注册此类读取时才会知晓，因此需在根目录 `Cargo.toml` 中声明这些 crate：

```toml
[workspace.metadata.mitts]          # [package.metadata.mitts] in a single crate
impure-proc-macros = ["sqlx", "askama", "rust-embed"]
```

依赖所列宏的 crate 会根据其 package 中的每个文件及其绝对路径来计算键：编辑模板会使它们重新构建，并且其他检出绝不会复用它们。所有其他宏均被视为纯宏（pure），依赖这些宏的 crate 可在不同 worktree 间共享。写明您所依赖的 crate 名称即可；它所引入的宏 crate（`sqlx-macros`）同样有效。

## 对比

各工具如何复用工作的定性对比。这并非性能声明。

| | Cargo | Cargo + sccache | Mitts |
| --- | --- | --- | --- |
| 原样读取 `Cargo.toml` / `Cargo.lock` | 是 | 是 | 是 |
| 跨工作区与 worktree 复用 | 否（按 `target/`） | 是，按 `rustc` 调用 | 是，按单元 |
| 键取决于检出路径 | — | 是，除非经标准化 | 注册源 crate 为否；工作区 crate 仅在不符合条件时为是 |
| 缓存 `bin`、测试与 `proc-macro` 单元 | — | 否 | 是 |
| 缓存 build-script 运行 | 否 | 否 | 是 |
| 同一依赖项的并发构建 | 等待 `target/` 锁 | 未命中时两者均可能编译 | 一个编译，另一个复用 |
| 远程 / 团队缓存 | 否 | 是（S3、GCS、Redis、…） | 计划中 |
| Windows | 是 | 是 | 否 |

Bazel 和 Buck2 解决的是更广泛的问题——带远程执行的封闭式（hermetic）多语言构建——但需要它们自己的构建文件，而不是 `Cargo.toml`。

<a id="limitations"></a>
## 局限性

Mitts 处于预发布阶段。在依赖它之前请先了解以下几点：

- **仅限 Linux 和 macOS。** 锁与 jobserver 均为 POSIX 机制。
- **仅限宿主目标架构。** 暂不支持交叉编译。
- **暂无远程缓存。** 存储为单机级别。
- **仅限 sparse 注册源。** 以 git 索引形式提供的注册源（除 crates.io 以外的 `registry+…`，或未带 `sparse+` 的 `[source] registry = "https://…"`）及 `local-registry` 源码均会被拒绝并提供相应建议。
- **凭据只读，从不写入。** `cargo login` 仍是 Cargo 的职责；`cargo:wincred` 仅限 Windows 且会被跳过。
- **部分单元永不缓存：** `cdylib`、`dylib` 或 `staticlib` 类型的 crate，以及 `env!` 读取无法追踪的 crate。它们始终重新构建；`mitts explain` 会对它们进行报告。
- **跨检出的工作区 crate。** 您自己的 crate 仅在它们的键可以做到路径无关（无 `CARGO_MANIFEST_DIR` 读取、无 build script、无声明的非纯 proc macro）时，才能在 worktree 间复用。否则键会包含绝对路径。在 macOS 上，链接器的调试 stabs 相对于工作区写入（`-oso_prefix`），因此链接生成的二进制文件也可以共享；在工作区根目录下启动调试器以便其查找对象文件。
- **读取文件的宏需由您自行声明。** 除非项目将其列入清单（见上文），否则 proc macro 均被视为纯宏。依赖已列出宏的 crate 会追踪其 package 内的每个文件；未列出但读取文件且未向 rustc 注册的宏可能会产生陈旧的结果，这与 Cargo 完全一致。
- **Build script 未经沙盒化。** 读取未经 `rerun-if-*` 声明的文件或变量的 script 可能会产生陈旧的结果——这与 Cargo 完全一致。
- **不受支持的 Cargo 特性会明确报错。** 例如 `--profile`，以及 `workspace.members` 中间目录包含通配符的 glob。Mitts 会报告它们，而不是盲目猜测。
- **同文件系统零拷贝。** 从暂存区进行零拷贝单元提升要求 `MITTS_CACHE_DIR` 与工作区 `target/` 目录位于同一文件系统挂载点上。跨越文件系统边界时会自动安全回退到标准复制。

如果您需要 Windows、交叉编译、团队缓存或稳定版本，**请暂勿使用 Mitts**。在这些场景下，Cargo（可选用 sccache）是稳妥之选。

<a id="performance"></a>
## 性能

基于 Mitts `0.3.17` 在 macOS（Apple M2 Ultra，APFS）与 Linux ARM64（XFS）上的 **100 个开源 Rust 项目**，以及 Linux x86_64（AMD Ryzen 9 9950X，XFS）上的 **90 个项目**实测，对比原生 `cargo` 与 `cargo` + `sccache`（`0.17.0`）。每项读数均为 3 次重复运行的中位数，并校验诊断信息、退出码与测试结果等价性；全部受测项目均计入汇总（分别有 `0`、`7`、`0` 个运行被标记为波动），且所有数字均不夸大实测优势：基线数据向下截断，Mitts 数据向上进位（[ROADMAP M8](../ROADMAP.md)）。完整研究报告、逐项目数据与测试方法见 [mittsbuild.com/the-hundred](https://mittsbuild.com/the-hundred/)。

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="../docs/img/bench-scenarios-dark.svg">
    <source media="(prefers-color-scheme: light)" srcset="../docs/img/bench-scenarios-light.svg">
    <img src="../docs/img/bench-scenarios-light.svg" width="920" alt="六种场景下中位数项目的构建耗时：Cargo、Cargo + sccache 与 Mitts">
  </picture>
</p>

每一行展示中位数项目的构建耗时（各工具在受测项目上的自身中位数，每行采用独立线性刻度），右侧同时列出 Mitts 最先完成的项目数量。在 macOS 上，中位数项目构建 12 个 worktree 在 `cargo` 下耗时 **32 s**，`cargo` + `sccache` 下耗时 **31 s**，Mitts 下耗时 **3.8 s**。在本地存储已预热的情况下，第二个干净 worktree 分别耗时 **1.7 s**、**1.6 s** 与 **79 ms**；分支切换耗时 **627 ms**、**209 ms** 与 **27 ms**；无改动构建耗时 **100 ms**、**41 ms** 与 **9 ms**。在这四种场景中，Mitts 在 100 个项目中的 99–100 个上最先完成（第二个 worktree 为 71 个中的 71 个）。

Mitts 未占优的场景：

- **冷构建，空缓存。** 原生 `cargo` 在 macOS 的 100 个项目中于 71 个上最先完成（Linux x86_64 为 90 个中的 60 个，Linux aarch64 为 100 个中的 77 个），在最大项目上最多快 6.2 s，原因是 Mitts 需要对输入计算哈希并填充空存储。中位数项目在 `cargo` 下耗时 **1.7 s**，在 Mitts 下耗时 **2.0 s**。Mitts 仍快于冷缓存的 `sccache`（**2.4 s**，Mitts 在 100 个中的 86 个上最先完成）。
- **修改单个文件。** 中位数项目在 `cargo` 下耗时 **364 ms**，`sccache` 下耗时 **596 ms**，Mitts 下耗时 **315 ms**，相对 `cargo` 时 Mitts 在 71 个项目中的 49 个上最先完成。其余 22 个项目（x86_64 为 64 个中的 21 个，aarch64 为 71 个中的 29 个）由 `cargo` 最先完成，在 macOS 上最多快 2.6 s，在 x86_64 上最多快 8.6 s，原因是 `cargo` 在大型 crate 内部复用了 `rustc` 增量编译，而 Mitts 以整个 crate 编译单元为粒度缓存。
- **重放 10 个提交**（未列入图表）：互有胜负，macOS 上 Mitts 在 100 个项目中的 58 个上最先完成，两者中位数接近（`cargo` 为 **5.6 s**，Mitts 为 **5.8 s**）。

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="../docs/img/bench-swarm-dark.svg">
    <source media="(prefers-color-scheme: light)" srcset="../docs/img/bench-swarm-light.svg">
    <img src="../docs/img/bench-swarm-light.svg" width="920" alt="100 个 Rust 项目上 12 个并行 worktree 构建计分板：耗时、CPU、编译器调用次数与磁盘写入量">
  </picture>
</p>

在全部项目上同时构建横跨 4 个提交的 **12 个并行 worktree**，各机器汇总结果如下：

| 机器 | 耗时：`cargo` / `sccache` / Mitts | `rustc` 调用次数：`cargo` / Mitts | 磁盘写入量：`cargo` / Mitts |
|---|---|---|---|
| macOS, APFS, M2 Ultra（100 个项目） | 76 min / 73 min / **13 min** | 90,961 / **6,905** | 641 GB / **112 GB** |
| Linux x86_64, XFS, Ryzen 9 9950X（90 个项目） | 60 min / 52 min / **9 min** | 75,049 / **6,162** | 651 GB / **106 GB** |
| Linux aarch64, XFS（100 个项目） | 78 min / 76 min / **22 min** | 90,544 / **7,348** | 743 GB / **121 GB** |

macOS 上的总 CPU 时间从 **20.3 h**（`cargo`）和 **8.0 h**（`sccache`）降至 **2.7 h**。x86_64 机器实测了 90 个项目而非 100 个：该轮运行缺少 10 个项目（similar, smallvec, smol, syntect, tealdeer, tera, textwrap, tokei, tower-http, xh），缺失原因未记录。


## 现状

| 领域 | 状态 |
| --- | --- |
| `Cargo.toml` / `Cargo.lock` 读取、sparse-index 解析 | 完成 |
| 特性解析（resolver 1 与 2） | 完成 |
| 并行获取器与全局源码存储 | 完成 |
| 原生 DAG 运行器、流水线化 `rustc`、jobserver | 完成 |
| 内容寻址存储与动作缓存 | 完成 |
| Build script 执行与缓存 | 完成 |
| 空操作快速路径 | 完成 |
| `check`、`clippy`、`fmt`、`test`（含文档测试）、`coverage` | 完成 |
| 具配额的保守 GC | 完成 |
| 基于同一存储的 C/C++ 适配器 | 实验性 |
| 工作区 crate 路径无关的键 | 部分支持 |
| 可复现基准测试研究 | 完成 |
| 远程 / 团队缓存 | 计划中 |
| Windows、交叉编译 | 不支持 |

里程碑与证明：[ROADMAP.md](../ROADMAP.md)。

## 常见问题

**Mitts 会取代 Cargo 吗？**
Mitts 读取现有的 Cargo 清单与 lockfile，并为本项目提供 Rust 构建/测试工作流。它自身的 CI 和发布工作流使用 Mitts 而不调用 Cargo。对于有需要的外部用户，Cargo 仍然可用。

**我可以在 CI 中使用它吗？**
可以。本代码仓库的 CI 会安装最新稳定版 Mitts，然后将其用于原生构建、测试、覆盖率、格式化和 Clippy。共享工件存储对于每个 runner 都是本地的；跨机器的远程缓存目前尚不可用。

**它会与 rust-analyzer 或现有的 Cargo 输出冲突吗？**
Mitts 使用与 Cargo 相同的默认输出路径。其共享工件存储保持独立，且 `mitts clean` 仅移除与记录内容仍相符的已索引 Mitts 输出；它会保留无关的或被替换的 Cargo 文件。如果 Cargo 持有某个 profile 的构建目录锁，清理操作将跳过，直至该构建完成。

**缓存放在哪里，如何清理？**
`--cache-dir`，否则为 `$MITTS_CACHE_DIR`、`$XDG_CACHE_HOME/mitts` 或 `~/.cache/mitts`；Mitts 缓存的所有内容均位于该单一根路径下。使用 `mitts gc` 在配额内进行清理；在没有构建运行时直接删除该目录也是安全的。

**为什么某个单元没有被缓存？**
运行 `mitts explain`。它会列出每个单元以及其符合或不符合缓存条件的具体原因。

## 设计原则

- 保持构建与缓存编排逻辑语言无关。语言语义保留在适配器中；Rust 是第一个适配器。
- 缓存命中需要完整且受追踪的输入。未知输入被视为已发生变化。
- 共享工件不可变且在使用前经过校验。可变状态保留在各个工作区中。不建立指向可写文件的硬链接，不搞时间戳花招。
- 清理操作绝不触碰正在进行的活跃构建的工件。

贡献者与 Agent 规则：[AGENTS.md](../AGENTS.md)。
架构决策：[docs/decisions](../docs/decisions)。

## 作者

[Alexander Panasenko](https://prod.codes/about/) · alex@prod.codes · [mittsbuild.com](https://mittsbuild.com)

## 引用

引用 Mitts 时，请使用已归档的发布版本。概念 DOI [10.5281/zenodo.24030067](https://doi.org/10.5281/zenodo.24030067) 始终指向最新版本；0.3.29 的版本 DOI 为 [10.5281/zenodo.24030068](https://doi.org/10.5281/zenodo.24030068)。GitHub 的「Cite this repository」会读取 [CITATION.cff](../CITATION.cff)。

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

## 许可证

基于 [MIT](../LICENSE-MIT) 或 [Apache-2.0](../LICENSE-APACHE) 双重许可，任您选择。
