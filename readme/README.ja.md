<!-- translation-source: 8cdc4c9b31d16dfc2a53f0fa412b8957113b5e1d -->

> [!NOTE]
> この文書はコミット [8cdc4c9](https://github.com/alex09x/mitts/blob/8cdc4c9b31d16dfc2a53f0fa412b8957113b5e1d/README.md) 時点の公式英語ドキュメントの翻訳です。最新の正式な内容は[英語版README](https://github.com/alex09x/mitts/blob/main/README.md)を参照してください。

<p align="center">
  <img src="../site/logo.svg" width="96" height="96" alt="Mitts Build logo">
</p>

<h1 align="center">Mitts Build</h1>

<p align="center">
  <b>ビルドの作業を、すべて正確に把握する。</b><br>
  マシン全体で共有されるコンテンツアドレス指定のアーティファクトキャッシュと、
  明示的な入力追跡を備えたRustファーストのビルドシステム。既存の <code>Cargo.toml</code> と <code>Cargo.lock</code> をそのまま読み込みます。
</p>

<p align="center">
  <a href="https://mittsbuild.com"><img src="https://img.shields.io/badge/site-mittsbuild.com-ff3b30" alt="Website"></a>
  <img src="https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-c9ced4" alt="License: MIT OR Apache-2.0">
  <img src="https://img.shields.io/badge/language-Rust-1b1d20?logo=rust" alt="Written in Rust">
  <img src="https://img.shields.io/badge/platform-Linux%20%7C%20macOS-1b1d20" alt="Platforms: Linux, macOS">
  <img src="https://img.shields.io/badge/status-pre--release-ff3b30" alt="Status: pre-release">
</p>

<p align="center">
  <a href="#quick-start">クイックスタート</a> ·
  <a href="#how-it-works">仕組み</a> ·
  <a href="#architecture">アーキテクチャ</a> ·
  <a href="../docs/architecture.md">プロトコル</a> ·
  <a href="#limitations">制限事項</a> ·
  <a href="../ROADMAP.md">ロードマップ</a>
</p>

<p align="center">
  <a href="https://github.com/alex09x/mitts/blob/main/README.md">English</a> ·
  <a href="https://github.com/alex09x/mitts/blob/main/readme/README.ru.md">Русский</a> ·
  <a href="https://github.com/alex09x/mitts/blob/main/readme/README.zh-CN.md">简体中文</a> ·
  <a href="https://github.com/alex09x/mitts/blob/main/readme/README.es.md">Español</a> ·
  <a href="https://github.com/alex09x/mitts/blob/main/readme/README.pt-BR.md">Português (Brasil)</a> ·
  <a href="https://github.com/alex09x/mitts/blob/main/readme/README.ja.md"><b>日本語</b></a> ·
  <a href="https://github.com/alex09x/mitts/blob/main/readme/README.de.md">Deutsch</a> ·
  <a href="https://github.com/alex09x/mitts/blob/main/readme/README.fr.md">Français</a> ·
  <a href="https://github.com/alex09x/mitts/blob/main/readme/README.ko.md">한국어</a> ·
  <a href="https://github.com/alex09x/mitts/blob/main/readme/README.ar.md">العربية</a>
</p>

---

Mittsはワークスペース自体を独自に計画し、`rustc`を直接呼び出します。すべての
コンパイルアクションは、そのすべての入力から導出されたキーを取得します。出力は
マシン全体で共有される不変ストアに一度だけ保存され、プロジェクト、ワークツリー、
ブランチを越えて、それを必要とするすべてのワークスペースへ復元されます。

入力を正確に把握できない場合、推測による再利用は行われず、アクションは再ビルドされます。

## 課題

Cargoのビルドキャッシュは、各ワークスペースの `target/` ディレクトリ内に配置されます。
新しく `target/` を作成する操作はすべてゼロからのスタートになります：

- 同じリポジトリの2つ目のチェックアウトや `git worktree`、
- `Cargo.lock` を変更するブランチへの切り替えと、その後の元のブランチへの復帰、
- 同じクレートに同じフィーチャーで依存している別のプロジェクト、
- それぞれ独自のワークツリーで稼働する並列コーディングエージェント。

同じ `serde`、`syn`、`tokio` が何度も繰り返しコンパイルされ、各コピーがディスク容量を消費します。

通常の回避策には注意すべき難点があります：

- **共有 `CARGO_TARGET_DIR`**: 単一のロックによってビルドが直列化され、フィーチャーセットが異なるプロジェクト同士がお互いのアーティファクトを上書きしてしまいます。
- **sccache などのコンパイララッパー**: 一度に1つの `rustc` 呼び出ししか認識できません。パスが正規化されていない限りキーは絶対パスに依存し、リンカを呼び出すクレート（`bin`、`proc-macro`、`dylib`）はキャッシュされません。さらに、Cargoがビルドの計画や各ビルドスクリプトの実行を引き続き行います。

Mittsは異なるアプローチをとります。コンパイラではなくCargoのエグゼキュータを置き換えます。ユニットグラフ全体を把握できるため、各ユニットをその依存関係のキーでキー指定し、リンクユニットやビルドスクリプトの実行をキャッシュして、複数のビルドが1つのストアを安全に共有できるようにします。

## 主な特徴

- **ワークスペースを越えた再利用。** マシンごとに1つのストア。あるチェックアウト向けにコンパイルされた依存関係は、同じ入力を持つ次のチェックアウトへ復元されます。
- **完全なキー。** ツールチェーン、ターゲット、プロファイル、フラグ、フィーチャー、ソース内容、依存関係のキー、および追跡された環境変数。未知の入力がある場合は再ビルドされます。
- **Cargoマニフェストがそのまま規約。** 新しいマニフェストも、2つ目のロックファイルも不要です。このリポジトリのビルド、テスト、lint、カバレッジ、およびリリースソースのワークフローはMittsを介して実行され、Cargo CLIを呼び出しません。外部におけるCargoの相互運用性も利用者のために引き続き提供されます。
- **リンクユニットとビルドスクリプトもキャッシュ。** `rlib` だけでなく、バイナリ、テスト、手続き型マクロ、ビルドスクリプトの実行もキー指定されて保存されます。
- **並行ビルドの協調動作。** キーごとのロック：同じ依存関係を必要とする2つのターミナルやワークツリーは、それを一度だけコンパイルします。1つのジョブサーバーがマシン上のすべてのMittsプロセスの並列度を制限します。
- **安全な共有。** ストアオブジェクトは不変です。アトミックに公開され、読み取り専用で保存され、reflinkまたはコピーによって復元されます。`MITTS_VERIFY_RESTORE=1` は復元前に対象オブジェクトを再度ハッシュ検証します（デフォルトではストアを信頼するため、復元されるバイトの再読み込みを1回分省略できます）。ファイルシステムがreflinkをサポートしていない場合、復元時はバイトをコピーする代わりに、読み取り専用のストアオブジェクトを `target/` にハードリンクします。共有inodeは `0o444`（実行可能ファイルの場合は `0o555`）のまま維持され、書き込みビットが付与されたリンクはターゲットインデックスによって拒否され、Mittsがワークスペースファイルを置換する際は必ずリネーム操作を用います。ストアオブジェクトへの書き込み可能なリンクが作成されることは決してありません。`MITTS_STORE_HARDLINKS=0` を指定すると、他のツールがその場で書き込みを行うワークスペース向けにプライベートなコピーを保持します。
- **ゼロコピーのワークスペースステージング。** ワークスペース内のライブラリエイリアス、セカンダリバイナリ、およびアトミックなユニットステージングでは、バイトをコピーすることなくワークスペース内ハードリンクとリネーム昇格を使用します。`MITTS_CACHE_DIR` とワークスペースの `target/` が同一のファイルシステムボリューム上にある場合、ステージングの昇格はバイトをコピーせずに行われます。異なるマウントをまたぐ場合は、自動的に安全なコピーへとフォールバックします。
- **no-op ファストパス。** 変更のないワークスペースでは、計画およびキー導出の処理をスキップします。
- **1つのCLI。** `build`、`run`、`check`、`clippy`、`fmt`、`test`（doctestを含む）、`coverage`、`explain`、`fetch`、`gc`、`clean`。

<a id="quick-start"></a>
## クイックスタート

最新の署名済みネイティブリリースをインストールします：

```sh
curl -fsSL https://mittsbuild.com/install.sh | sh
```

ソースチェックアウトからビルドするには、まずリリース版の最新Mittsをインストールし、次にMittsを使ってチェックアウトをビルドします：

```sh
git clone https://github.com/alex09x/mitts
cd mitts
mitts build --release
MITTS_JOBS=2 mitts build     # limit concurrent native build units
install -m755 target/release/mitts "$HOME/.cargo/bin/mitts"
```

その後、任意のCargoワークスペース内で実行します：

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

Gitプロジェクトの場合、パッケージングには現在、ルートにパッケージが存在するクリーンなリポジトリが必要です。マニフェストの `include` パターンによって無視対象のファイルを選択できます。ネストされたワークスペースパッケージやシンボリックリンクされたディレクトリはまだサポートされていません。`mitts publish` は crates.io を対象とし、`CARGO_REGISTRY_TOKEN` からトークンを読み取ります。

Mittsはビルド出力をCargoの通常の `target/debug/` および `target/release/` ディレクトリに書き込みます。`CARGO_TARGET_DIR` および `--target-dir` で別の出力ルートを選択できます。共有キャッシュは分離されています（`--cache-dir`、未指定なら `$MITTS_CACHE_DIR`、未指定なら `$XDG_CACHE_HOME/mitts`、未指定なら `~/.cache/mitts`）。アーティファクトストア、アクションキャッシュ、ロック、ジョブサーバー、ソースストア、gitミラー、インデックスキャッシュのすべてが、その単一のルートの下に配置されます。

Git依存関係は、パブリックかプライベートかを問わず、システムの `git` を使用してMitts自体によってクローンされます。そのため、SSHキー、エージェント、認証情報ヘルパー、および `GIT_SSH_COMMAND` はコマンドラインとまったく同様に適用されます。対話プロンプトは無効化されているため、認証情報が見つからない場合はハングせずにエラーとなります。Cargoを経由するものは一切ありません。Cargoがすでに `~/.cargo/git/checkouts` 配下に作成したチェックアウトが存在する場合は、再クローンせずにそれが再利用されます。

Cargoがクレートを取得する他のすべての方法も、同じ設定から同様に機能します：

- **代替レジストリ**: 依存関係で `[registries.<name>] index = "sparse+https://…"` および `registry = "<name>"` を指定。トークンはCargoの認証情報プロバイダからCargoと同じ優先順位で取得されます: `cargo:token`（環境変数および `credentials.toml`）、`cargo:macos-keychain`、`cargo:libsecret`、`cargo:token-from-stdout <command>`、および `[registry] global-credential-providers`、`[registries.<name>] credential-provider`、`[credential-alias]` で設定された認証情報プロバイダプロトコル経由のプラグイン（`cargo-credential-1password` など）。`config.json` に `auth-required` と記載されているレジストリで、どのプロバイダにもトークンがない場合は明確なメッセージを表示して失敗します。Gitインデックスレジストリは拒絶されます: sparseプロトコルを使用してください。
- **ミラー**: `[source.crates-io] replace-with = "<name>"` および `[source.<name>] registry = "sparse+https://…"`. バイトデータはミラーから取得され、`Cargo.lock` にはCargoが記述するのとまったく同様にcrates.ioの名前が保持されます。
- **ベンダリングされたソース**: `cargo vendor` による `[source.vendored-sources] directory = "vendor"`。パッケージは `.cargo-checksum.json` に照らして検証され、ストアにコピーされます。gitパッケージを含め、ネットワークには一切アクセスしません。ロックファイルがない場合、vendorディレクトリがインデックスとして機能します。
- **`[patch]`**: `[patch.crates-io] foo = { path = … }` または `{ git = … }` がレジストリパッケージを置き換えます。ロックファイルがない場合、置き換え対象が解決されて固定され、Cargoと同様に要求を満たす必要があります。

Cargo設定内の `[env]` は、Cargoの `force` および `relative` ルールに従って、rustc、ビルドスクリプト、ならびに `mitts run` および `mitts test` が起動するプログラムに適用されます。設定ファイルは入力の一部であるため、値が変更されるとそれを読み取った対象が再ビルドされます。

`Cargo.lock` が存在しない場合、MittsはCargoと同じ方法で解決します：要求ごとにヤンクされていない最新バージョンを選択し、エッジに沿ってフィーチャーを伝播させ、フィーチャーによって有効化された場合のみオプショナルな依存関係を含め、互換性のある範囲ごとに1つのバージョンを選択します。結果として生成されるロックファイルは、Cargoが自身のものとして受け入れ可能なものになります。

<a id="how-it-works"></a>
## 仕組み

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

ユニットのキーには依存関係の出力バイトではなく依存関係の**キー**が含まれるため、ユニットの下位のどこかで変更があればそのキーも変化します。ビルドスクリプトはステージングディレクトリ内で実行されます。その指示内容（`rustc-cfg`、`rustc-link-*`、`rustc-env` など）は設定対象ユニットのキーに組み込まれ、`rerun-if-*` 宣言によってキャッシュされた実行結果を再利用できるかどうかが判断されます。

ビルドが成功した後、Mittsはワークスペースのスナップショットを記録します。ワークスペース内の何も変更されていない場合、次回のビルドは即座に結果へとスキップします。不一致があった場合は完全なキー指定ビルドへとフォールバックするため、プロトコルが実行すべき作業をファストパスがスキップしてしまうことは決してありません。

キーの導出、ロック、ビルドスクリプト、ファストパス、ストアのレイアウト、ガベージコレクションを含む完全なプロトコルは [docs/architecture.md](../docs/architecture.md) に記載されています。

<a id="architecture"></a>
## アーキテクチャ

Mittsは言語ニュートラルなコアとその上に位置する言語アダプターで構成されています。コアはCargoについて何も認識しません。RustアダプターがCargoワークスペースをキー付きのアクションへと変換します。

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

| パス | 内容 |
| --- | --- |
| `src/cli/` | コマンドハンドラー: `build`、`test`、`clippy`、`explain` など |
| `src/adapter/rust/` | Cargoセマンティクス: マニフェスト、ロックファイル、sparseインデックス、フィーチャー解決、ユニット計画、アクションキー、DAGランナー、ビルドスクリプト、doctest |
| `src/adapter/c/` | 同じストアを利用する実験的C/C++アダプター |
| `src/core/` | 言語ニュートラルなストア（`cas.rs`）、アクションキャッシュ、ロック、ジョブサーバー、ファストパス（`build_lock*.rs`）、GC、reflink |
| `docs/` | [プロトコル](../docs/architecture.md)、[設計上の決定](../docs/decisions)、[シナリオ](../docs/scenarios) |
| `tests/` | Cargo同等性テストおよびキャッシュ無効化テストを含む統合テスト |
| `scripts/` | ベンチマークおよび監査ハーネス |
| `fixtures/` | テストで使用される最小限のRustおよびCプロジェクト |

## キャッシュが無効化される条件

キャッシュヒットには、以下のすべての入力が一致する必要があります。Mittsが入力を確認できない場合、結果は再利用されません。

| 入力 | 追跡方法 |
| --- | --- |
| ツールチェーン | `rustc -vV` の出力 |
| ターゲットとプロファイル | ターゲットトリプル、プロファイル設定、`RUSTFLAGS`、およびCargo設定のrustflags |
| フィーチャー | ユニットの解決されたフィーチャーセット（Cargoと同様にリゾルバー1または2） |
| ソース | パッケージファイルおよびrustcが読み取りを報告したすべてのファイル（`include_str!`、`include_bytes!`、モジュール）のコンテンツハッシュ |
| 依存関係 | すべての依存ユニットのアクションキー |
| 環境変数 | 関連する `CARGO_*` および `RUST_*` 設定、ならびに `env!` / `option_env!` を通じて読み取られた変数（`<unset>` も明示的に記録） |
| Cargo設定 | Cargoが読み取る `.cargo/config(.toml)` ファイル群 |
| ビルドスクリプト | その指示内容、`OUT_DIR`、ならびに `rerun-if-changed` パスの内容および `rerun-if-env-changed` 変数の値 |

設定された `rustc-wrapper` および `rustc-workspace-wrapper` プログラムはコンパイル時に呼び出されます。任意のラッパーは追跡されていない入力に依存する可能性があるため、いずれかのラッパーがアクティブな間、Mittsはアクションキャッシュおよびビルドスクリプトキャッシュの再利用を無効化します。これによりビルドが遅くなる場合があります。

一部の手続き型マクロは、展開中にパッケージからファイルを読み取ります：`sqlx` は `.sqlx/` に対してクエリをチェックし、`askama` は `templates/` をコンパイルし、`rust-embed` や `include_dir` はディレクトリ全体を埋め込みます。マクロがそれらを登録しない限り、rustcはそのような読み取りを認識できないため、ルートの `Cargo.toml` でこれらのクレートを宣言してください：

```toml
[workspace.metadata.mitts]          # [package.metadata.mitts] in a single crate
impure-proc-macros = ["sqlx", "askama", "rust-embed"]
```

リストされたマクロに依存するクレートは、そのパッケージ内のすべてのファイルと絶対パスに基づいてキー指定されます。テンプレートが編集されると再ビルドされ、別のチェックアウトで再利用されることはありません。それ以外のマクロは純粋（pure）として扱われ、その依存先はワークツリー間で共有されます。依存しているクレート名を指定してください。それが引き込むマクロクレート（`sqlx-macros`）を指定しても機能します。

## 比較

各ツールがどのように作業を再利用するかについての定性的な比較です。パフォーマンスを主張するものではありません。

| | Cargo | Cargo + sccache | Mitts |
| --- | --- | --- | --- |
| `Cargo.toml` / `Cargo.lock` をそのまま読み込む | はい | はい | はい |
| ワークスペースやワークツリーを越えた再利用 | いいえ（`target/` ごと） | はい（`rustc` 呼び出しごと） | はい（ユニットごと） |
| キーがチェックアウトパスに依存するか | — | はい（正規化されていない限り） | レジストリクレートはいいえ。ワークスペースクレートは対象外の場合のみはい |
| `bin`、テスト、`proc-macro` ユニットをキャッシュするか | — | いいえ | はい |
| ビルドスクリプトの実行をキャッシュするか | いいえ | いいえ | はい |
| 同一依存関係の並行ビルド | `target/` ロックで待機 | ミス時に双方がコンパイルする可能性あり | 一方がコンパイルし、他方は再利用 |
| リモート / チームキャッシュ | いいえ | はい（S3、GCS、Redisなど） | 計画中 |
| Windows | はい | はい | いいえ |

BazelやBuck2はより大きな課題（リモート実行を伴う密閉型・多言語ビルド）を解決しますが、`Cargo.toml` の代わりに独自のビルドファイルを必要とします。

<a id="limitations"></a>
## 制限事項

Mittsはプレリリース段階です。依存する前に以下を把握してください：

- **LinuxおよびmacOSのみ。** ロックおよびジョブサーバーはPOSIX仕様です。
- **ホストターゲットのみ。** クロスコンパイルはまだサポートされていません。
- **リモートキャッシュは未提供。** ストアはマシン単位です。
- **sparseレジストリのみ。** gitインデックスとして提供されるレジストリ（crates.io以外の `registry+…`、または `sparse+` のない `[source] registry = "https://…"`）および `local-registry` ソースはアドバイスとともに拒否されます。
- **認証情報は読み取りのみで、書き込みは行いません。** `cargo login` は引き続きCargoの役割です。`cargo:wincred` はWindows専用のためスキップされます。
- **一部のユニットは決してキャッシュされません:** `cdylib`、`dylib`、または `staticlib` タイプのクレート、および `env!` の読み取りを追跡できないクレート。これらは常にビルドされます。`mitts explain` によって報告されます。
- **チェックアウトをまたぐワークスペースクレート。** 自身のクレートがワークツリー間で再利用されるのは、そのキーをパス非依存にできる場合（`CARGO_MANIFEST_DIR` の読み取りがなく、ビルドスクリプトがなく、宣言された impure な手続き型マクロがない場合）に限られます。それ以外の場合、キーには絶対パスが含まれます。macOSではリンカのデバッグスタブがワークスペース相対（`-oso_prefix`）で書き込まれるため、リンクされたバイナリも共有されます。デバッガがオブジェクトファイルを見つけられるように、ワークスペースのルートでデバッガを起動してください。
- **ファイルを読み取るマクロの宣言は利用者の責任です。** プロジェクトがリストに明記しない限り、手続き型マクロは純粋なものとして扱われます（上記参照）。リストされたマクロの依存先はパッケージ内のすべてのファイルに従います。rustcに登録せずにファイルを読み取る未登録のマクロは、Cargoとまったく同様に古い結果を生成する可能性があります。
- **ビルドスクリプトはサンドボックス化されていません。** `rerun-if-*` で宣言しなかったファイルや変数を読み取るスクリプトは、Cargoとまったく同様に古い結果を生成する可能性があります。
- **未サポートのCargo機能は明確にエラーとなります。** 例えば `--profile` や、`workspace.members` 内の中間ディレクトリにあるワイルドカードを含むグロブなどです。Mittsは推測することなくそれらを報告します。
- **同一ファイルシステムでのゼロコピー。** ステージングからのゼロコピーのユニット昇格には、`MITTS_CACHE_DIR` とワークスペースの `target/` ディレクトリが同一のファイルシステムマウント上にある必要があります。ファイルシステムの境界をまたぐ場合は、安全に標準コピーへとフォールバックします。

Windows、クロスコンパイル、チームキャッシュ、または安定版リリースが必要な場合は、**まだMittsを使用しないでください**。その場合は、必要に応じてsccacheを組み合わせたCargoが安全な選択肢です。

## パフォーマンス

パフォーマンス測定値は、再現可能な方法論（固定されたハードウェア、コールド／ウォーム／入力変更時の実行、複数のワークツリー、ベースラインとしてのプレーンなCargoまたはsccacheのうちより強力な方、ならびに診断・終了コード・テスト結果の同等性確認）が整った場合にのみ公開されます。この検証は現在進行中です（[ROADMAP M8](../ROADMAP.md)）。それが公開されるまで、Mittsは高速化やディスク容量節約に関するいかなる主張も行いません。

## 状況

| 領域 | 状態 |
| --- | --- |
| `Cargo.toml` / `Cargo.lock` の読み込み、sparseインデックス解決 | 完了 |
| フィーチャー解決（リゾルバー1および2） | 完了 |
| 並列フェッチャーおよびグローバルソースストア | 完了 |
| ネイティブDAGランナー、パイプライン化された `rustc`、ジョブサーバー | 完了 |
| コンテンツアドレス指定ストアおよびアクションキャッシュ | 完了 |
| ビルドスクリプトの実行とキャッシング | 完了 |
| no-op ファストパス | 完了 |
| `check`、`clippy`、`fmt`、`test`（doctestを含む）、`coverage` | 完了 |
| クォータ付きの保守的GC | 完了 |
| 同じストアを利用するC/C++アダプター | 実験的 |
| ワークスペースクレートのパス非依存キー | 部分的 |
| 再現可能なベンチマーク検証 | 進行中 |
| リモート / チームキャッシュ | 計画中 |
| Windows、クロスコンパイル | 未サポート |

マイルストーンと実績: [ROADMAP.md](../ROADMAP.md)。

## FAQ

**MittsはCargoを置き換えるものですか？**
Mittsは既存のCargoマニフェストとロックファイルを読み込み、このプロジェクトにRustのビルド／テストワークフローを提供します。プロジェクト独自のCIおよびリリースワークフローでは、Cargoを呼び出すことなくMittsを使用しています。Cargoを必要とする外部ユーザーも引き続き利用できます。

**CIで使用できますか？**
はい。このリポジトリのCIでは最新の安定版Mittsをインストールし、ネイティブビルド、テスト、カバレッジ、フォーマット、およびClippyに使用しています。共有アーティファクトストアは各ランナーにローカルです。マシン間をまたぐリモートキャッシュはまだ利用できません。

**rust-analyzerや既存のCargo出力と衝突しませんか？**
MittsはCargoと同じデフォルトの出力パスを使用します。共有アーティファクトストアは分離されており、`mitts clean` は記録された内容と一致しているインデックス付きのMitts出力のみを削除します。無関係な、または置き換えられたCargoファイルは保持されます。Cargoがプロファイルのビルドディレクトリロックを保持している場合、そのビルドが終了するまでクリーンアップはスキップされます。

**キャッシュはどこにあり、どのようにクリアしますか？**
`--cache-dir`、未指定なら `$MITTS_CACHE_DIR`、`$XDG_CACHE_HOME/mitts`、または `~/.cache/mitts` です。Mittsがキャッシュするものはすべてその単一のルートの下に配置されます。クォータ内でプルーニングを行うには `mitts gc` を使用してください。ビルドが実行されていない状態であれば、ディレクトリを削除しても安全です。

**ユニットがキャッシュされなかったのはなぜですか？**
`mitts explain` を実行してください。すべてのユニットと、それらがキャッシュの対象である、または対象外である理由が一覧表示されます。

## 設計原則

- ビルドおよびキャッシュのオーケストレーションは言語ニュートラルに保つ。言語セマンティクスはアダプター内に配置し、Rustはその最初のアダプターです。
- キャッシュヒットには完全かつ追跡された入力が必要。未知の入力は変更されたものとして扱われます。
- 共有アーティファクトは不変であり、使用前に検証される。ミュータブルな状態は各ワークスペース内に保持する。書き込み可能なファイルへのハードリンクや、タイムスタンプによる小細工は行いません。
- クリーンアップがアクティブなビルドのアーティファクトに触れることは決してありません。

コントリビューターおよびエージェント向けルール: [AGENTS.md](../AGENTS.md)。
アーキテクチャの決定事項: [docs/decisions](../docs/decisions)。

## 著者

[Alexander Panasenko](https://prod.codes/about/) · alex@prod.codes ·
[mittsbuild.com](https://mittsbuild.com)

## ライセンス

利用者の選択に応じて、[MIT](../LICENSE-MIT) または [Apache-2.0](../LICENSE-APACHE) のデュアルライセンスです。
