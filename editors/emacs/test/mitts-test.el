;;; mitts-test.el --- Tests for mitts.el  -*- lexical-binding: t; -*-

;; Mitts Build for Emacs — editor integration for the Mitts Rust build tool
;; Copyright (c) 2026 Alexander Panasenko
;;
;; Contact: alex@prod.codes
;; Author: https://prod.codes/about/
;; Project: https://github.com/alex09x/mitts
;; SPDX-License-Identifier: MIT OR Apache-2.0

;;; Commentary:

;;   emacs --batch -L editors/emacs -l editors/emacs/test/mitts-test.el -f ert-run-tests-batch-and-exit
;; With MITTS_EMACS_E2E=1 and Mitts on PATH, `mitts-check' runs on a broken project too.

;;; Code:

(require 'ert)
(require 'cl-lib)
(require 'seq)
(require 'mitts)

(defconst mitts-test-dir
  (file-name-directory (or load-file-name buffer-file-name)))

(defun mitts-test--problems ()
  "The compilation messages of the current buffer: (TYPE FILE LINE COLUMN TEXT)."
  (compilation--ensure-parse (point-max))
  (let ((pos (point-min)) found)
    (while (setq pos (next-single-property-change pos 'compilation-message))
      (let ((msg (get-text-property pos 'compilation-message)))
        (when msg
          (let* ((loc (compilation--message->loc msg))
                 (file (caar (compilation--loc->file-struct loc))))
            (push (list (pcase (compilation--message->type msg) (2 'error) (1 'warning) (_ 'info))
                        (and file (replace-regexp-in-string "\\\\" "/" file))
                        (compilation--loc->line loc)
                        (compilation--loc->col loc)
                        (save-excursion
                          (goto-char pos)
                          (buffer-substring-no-properties
                           (line-beginning-position) (line-end-position))))
                  found)))
        ;; Skip to the end of this message's span.
        (setq pos (or (next-single-property-change pos 'compilation-message) (point-max)))))
    (nreverse found)))

(ert-deftest mitts-errors-and-warnings-from-mitts-output ()
  "Captured Mitts output: a warning and two errors, with Mitts' own lines around them."
  (with-temp-buffer
    (insert-file-contents (expand-file-name "check.txt" mitts-test-dir))
    (compilation-mode)
    (let ((problems (mapcar (lambda (p) (butlast p)) (mitts-test--problems))))
      (should (equal problems
                     '((warning "crates/a/src/main.rs" 2 9)
                       (error "crates/a/src/lib.rs" 1 27)
                       (error "crates/a/src/lib.rs" 3 20)))))))

(ert-deftest mitts-own-pattern-for-emacs-before-29 ()
  "The pattern mitts.el adds where Emacs has no `rust' one reads the same output."
  (let ((compilation-error-regexp-alist-alist
         `((mitts ,mitts--rustc-regexp 4 5 6 (2 . 3))))
        (compilation-error-regexp-alist '(mitts)))
    (with-temp-buffer
      (insert-file-contents (expand-file-name "check.txt" mitts-test-dir))
      (compilation-mode)
      (should (equal (mapcar #'butlast (mitts-test--problems))
                     '((warning "crates/a/src/main.rs" 2 9)
                       (error "crates/a/src/lib.rs" 1 27)
                       (error "crates/a/src/lib.rs" 3 20)))))))

(ert-deftest mitts-summaries-are-not-problems ()
  (with-temp-buffer
    (insert "error: aborting due to 2 previous errors\n"
            "warning: 1 warning emitted\n"
            "  × Failed to compile a v0.1.0 [lib:a]\n"
            "   ╭─[/abs/ws/crates/a/src/lib.rs:1:27]\n")
    (compilation-mode)
    (should (null (mitts-test--problems)))))

(ert-deftest mitts-compile-runs-from-the-workspace-root ()
  (let* ((ws (make-temp-file "mitts-ws" t))
         (member (expand-file-name "crates/a/src" ws))
         ran)
    (unwind-protect
        (progn
          (make-directory member t)
          (with-temp-file (expand-file-name "Cargo.toml" ws)
            (insert "[workspace]\nmembers = [\"crates/a\"]\n"))
          (with-temp-file (expand-file-name "crates/a/Cargo.toml" ws)
            (insert "[package]\nname = \"a\"\nversion = \"0.1.0\"\n"))
          (cl-letf (((symbol-function 'compile)
                     (lambda (command &rest _) (setq ran (cons command default-directory)))))
            (let ((default-directory (file-name-as-directory member))
                  (mitts-executable "/opt/my tools/mitts"))
              (mitts-check)))
          (should (equal (car ran)
                         (concat (shell-quote-argument "/opt/my tools/mitts") " check")))
          (should (equal (file-truename (cdr ran))
                         (file-truename (file-name-as-directory ws)))))
      (delete-directory ws t))))

(ert-deftest mitts-workspace-root-of-a-single-package ()
  (let ((pkg (make-temp-file "mitts-pkg" t)))
    (unwind-protect
        (progn
          (make-directory (expand-file-name "src" pkg))
          (with-temp-file (expand-file-name "Cargo.toml" pkg)
            (insert "[package]\nname = \"p\"\nversion = \"0.1.0\"\n"))
          (should (equal (file-truename (mitts-workspace-root (expand-file-name "src" pkg)))
                         (file-truename (file-name-as-directory pkg)))))
      (delete-directory pkg t))))

(ert-deftest mitts-workspace-root-is-the-nearest-workspace ()
  "A package with its own [workspace] inside another workspace is its own root, as
under Cargo (here without Mitts installed, by the manifests)."
  (let ((outer (make-temp-file "mitts-outer" t))
        (mitts-executable "mitts-not-installed-here"))
    (unwind-protect
        (let ((inner (expand-file-name "fixtures/inner" outer)))
          (make-directory (expand-file-name "src" inner) t)
          (with-temp-file (expand-file-name "Cargo.toml" outer)
            (insert "[workspace]\nmembers = [\".\"]\n[package]\nname = \"o\"\n"))
          (with-temp-file (expand-file-name "Cargo.toml" inner)
            (insert "[package]\nname = \"i\"\n\n[workspace]\n"))
          (should (equal (file-truename (mitts-workspace-root (expand-file-name "src" inner)))
                         (file-truename (file-name-as-directory inner)))))
      (delete-directory outer t))))

(defvar rust-cargo-bin)
(defvar rustic-cargo-bin)

(ert-deftest mitts-cargo-mode-sets-and-restores-rust-mode-and-rustic ()
  (let ((rust-cargo-bin "cargo"))
    (makunbound 'rustic-cargo-bin)
    (unwind-protect
        (progn
          (mitts-cargo-mode 1)
          (should (equal rust-cargo-bin "mitts"))
          (should (equal rustic-cargo-bin "mitts"))
          (mitts-cargo-mode -1)
          (should (equal rust-cargo-bin "cargo"))
          (should-not (boundp 'rustic-cargo-bin)))
      (when mitts-cargo-mode (mitts-cargo-mode -1)))))

(defvar eglot-workspace-configuration)

(ert-deftest mitts-eglot-setup-merges-the-rust-analyzer-settings ()
  (let ((eglot-workspace-configuration
         '(:rust-analyzer (:cargo (:features "all")) :gopls (:staticcheck t))))
    (mitts-eglot-setup)
    (let ((ra (plist-get eglot-workspace-configuration :rust-analyzer)))
      (should (equal (plist-get (plist-get ra :cargo) :features) "all"))
      (should (equal (plist-get (plist-get ra :check) :overrideCommand)
                     ["mitts" "check" "--workspace" "--all-targets" "--message-format=json"]))
      (should (equal (plist-get (plist-get ra :runnables) :command) "mitts")))
    (should (equal (plist-get eglot-workspace-configuration :gopls) '(:staticcheck t)))))

(defvar lsp-rust-analyzer-cargo-override-command)

(ert-deftest mitts-lsp-setup-sets-the-check-command ()
  (let ((lsp-rust-analyzer-cargo-override-command []))
    (mitts-lsp-setup)
    (should (equal lsp-rust-analyzer-cargo-override-command
                   ["mitts" "check" "--workspace" "--all-targets" "--message-format=json"]))))

(ert-deftest mitts-e2e-check-links-the-type-error ()
  "`mitts-check' on a broken project: its type error is a link to src/main.rs:2:18."
  (skip-unless (equal (getenv "MITTS_EMACS_E2E") "1"))
  (should (executable-find "mitts"))
  (let ((default-directory (expand-file-name "fixtures/broken/src/" mitts-test-dir))
        (done nil))
    (let ((compilation-finish-functions (list (lambda (&rest _) (setq done t)))))
      (mitts-check)
      (with-timeout (300 (error "mitts check did not finish"))
        (while (not done) (accept-process-output nil 0.2))))
    (with-current-buffer "*compilation*"
      (let ((errors (seq-filter (lambda (p) (eq (car p) 'error)) (mitts-test--problems))))
        (should (equal (mapcar #'butlast errors) '((error "src/main.rs" 2 18))))))))

(provide 'mitts-test)
;;; mitts-test.el ends here
