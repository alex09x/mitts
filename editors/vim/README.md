# Mitts for Vim and Neovim

Runs [Mitts](https://github.com/alex09x/mitts) from Vim and Neovim and puts rustc's errors
and warnings into the quickfix list. In Neovim it can also route rust-analyzer's on-save
check through Mitts.

## Install

The plugin is the `editors/vim` directory of the Mitts repository.

lazy.nvim:

```lua
{
  "alex09x/mitts",
  config = function(plugin)
    vim.opt.rtp:append(plugin.dir .. "/editors/vim")
    vim.opt.rtp:append(plugin.dir .. "/editors/vim/after")
    vim.cmd("runtime plugin/mitts.vim")
    require("mitts").setup()
  end,
}
```

vim-plug:

```vim
Plug 'alex09x/mitts', { 'rtp': 'editors/vim' }
```

Vim packages, by hand:

```sh
git clone https://github.com/alex09x/mitts ~/src/mitts
mkdir -p ~/.vim/pack/mitts/start
ln -s ~/src/mitts/editors/vim ~/.vim/pack/mitts/start/mitts
```

Mitts itself must be on `PATH` (or set `g:mitts_executable`).

## Use

| | |
|---|---|
| `:Mitts check`, `:Mitts build --release`, `:Mitts test` | runs Mitts through `:make`; problems go to the quickfix list (`:copen`) |
| `:Mitts! {args}` | the same, without jumping to the first problem |
| `:compiler mitts` then `:make check` | the compiler plugin on its own |

Rust buffers get `:compiler mitts` when Mitts is on `PATH`, so plain `:make check` runs
Mitts. Paths in the output are relative to the workspace root, as with Cargo: run `:make`
from there.

## Neovim and rust-analyzer

`require("mitts").setup()` (Neovim 0.11+) adds to the `rust_analyzer` configuration of
`vim.lsp.config`:

- `check.overrideCommand = { "mitts", "check", "--workspace", "--all-targets", "--message-format=json" }`
- `runnables.command = "mitts"`

It is merged with the configuration you already have. rust-analyzer still loads the
project with `cargo metadata` and runs build scripts with Cargo; only the check and the
runnables go through Mitts.

With nvim-lspconfig's older `setup{}` or with rustaceanvim, take the table from
`require("mitts").rust_analyzer_settings()` and pass it as `settings`.

## Options

| | default | |
|---|---|---|
| `g:mitts_executable` | `"mitts"` | the Mitts command |
| `g:mitts_set_compiler` | `1` | `0`: Rust buffers keep their own compiler |
| `require("mitts").setup({ rust_analyzer = false })` | | leave rust-analyzer alone |

## Tests

```sh
vim -Nu NONE -i NONE -es -S editors/vim/test/run.vim
nvim --headless -u NONE -i NONE -S editors/vim/test/run.vim
```

The result is in `editors/vim/test/result.txt`. With `MITTS_VIM_E2E=1` the tests also run
`:Mitts! check` on a broken project. CI runs both editors on Linux, macOS and Windows
(`.github/workflows/editors.yml`).
