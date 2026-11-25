" Mitts Build for Vim and Neovim — editor integration for the Mitts Rust build tool
" Copyright (c) 2026 Alexander Panasenko
"
" Contact: alex@prod.codes
" Author: https://prod.codes/about/
" Project: https://github.com/alex09x/mitts
" SPDX-License-Identifier: MIT OR Apache-2.0

" The commands Mitts takes, as Cargo's.
let s:commands = ['add', 'bench', 'build', 'check', 'clean', 'clippy', 'doc', 'fetch',
      \ 'fix', 'fmt', 'generate-lockfile', 'info', 'init', 'install', 'locate-project',
      \ 'metadata', 'new', 'package', 'pkgid', 'publish', 'remove', 'run', 'rustc',
      \ 'rustdoc', 'search', 'test', 'tree', 'uninstall', 'update', 'vendor', 'version']

function! mitts#complete(arglead, cmdline, cursorpos) abort
  " Only the first argument is a command.
  if a:cmdline[: a:cursorpos - 1] =~# '\v^\s*\S+\s+\S+\s'
    return []
  endif
  return filter(copy(s:commands), 'v:val =~# "^" . a:arglead')
endfunction

" Runs `mitts {args}` with :make in the current buffer's compiler settings, then puts the
" buffer's own back.
function! mitts#run(args, nojump) abort
  let l:saved = [&l:makeprg, &l:errorformat, get(b:, 'current_compiler', '')]
  try
    compiler mitts
    execute (a:nojump ? 'make!' : 'make') . ' ' . a:args
  finally
    let [&l:makeprg, &l:errorformat] = l:saved[0:1]
    if l:saved[2] ==# ''
      silent! unlet b:current_compiler
    else
      let b:current_compiler = l:saved[2]
    endif
  endtry
endfunction
