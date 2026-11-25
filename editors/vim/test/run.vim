" Mitts Build for Vim and Neovim — editor integration for the Mitts Rust build tool
" Copyright (c) 2026 Alexander Panasenko
"
" Contact: alex@prod.codes
" Author: https://prod.codes/about/
" Project: https://github.com/alex09x/mitts
" SPDX-License-Identifier: MIT OR Apache-2.0
"
" Runs every Test_* function of test/test_*.vim with the plugin on 'runtimepath':
"   vim -Nu NONE -i NONE -es -S editors/vim/test/run.vim
"   nvim --headless -u NONE -i NONE -S editors/vim/test/run.vim
" Writes the result to test/result.txt and exits non-zero when a test failed.

set nocompatible
let s:test_dir = expand('<sfile>:p:h')
let s:root = fnamemodify(s:test_dir, ':h')
execute 'set runtimepath^=' . fnameescape(s:root)
execute 'set runtimepath+=' . fnameescape(s:root . '/after')
filetype plugin on
runtime plugin/mitts.vim

for s:file in glob(s:test_dir . '/test_*.vim', 0, 1)
  execute 'source ' . fnameescape(s:file)
endfor

let s:lines = []
let s:failed = 0
let s:tests = sort(map(split(execute('function /^Test_'), "\n"), 'matchstr(v:val, ''Test_\w\+'')'))
for s:name in s:tests
  let v:errors = []
  try
    call call(s:name, [])
  catch
    call add(v:errors, 'exception: ' . v:exception . ' at ' . v:throwpoint)
  endtry
  if empty(v:errors)
    call add(s:lines, 'ok   ' . s:name)
  else
    let s:failed += 1
    call add(s:lines, 'FAIL ' . s:name)
    call extend(s:lines, map(copy(v:errors), '"       " . v:val'))
  endif
  " Each test starts from a fresh buffer.
  silent! %bwipeout!
endfor
call add(s:lines, (has('nvim') ? 'nvim' : 'vim') . ': ' . len(s:tests) . ' tests, ' . s:failed . ' failed')
call writefile(s:lines, s:test_dir . '/result.txt')
if s:failed
  cquit!
endif
qall!
