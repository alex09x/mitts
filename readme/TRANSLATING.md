# Maintaining the README translations

[The English README](../README.md) is the canonical source. The localized READMEs
translate its explanations and navigation. Code, commands, recorded CLI output and
diagrams remain identical to the English fenced blocks.

The first set covers Russian (`ru`), Simplified Chinese (`zh-CN`), Spanish (`es`),
Brazilian Portuguese (`pt-BR`), Japanese (`ja`), German (`de`), French (`fr`),
Korean (`ko`) and Arabic (`ar`), alongside English.

When changing the English README:

1. Update the corresponding explanations in every translation. Keep the distinction
   between measured facts and limitations; never turn a stated limit into a claim.
2. Copy changed fenced blocks directly from English. Keep addresses, command flags,
   versions and real output intact.
3. Update the `translation-source` comment to the English commit reviewed and update the
   localized source link. A source marker records a review snapshot; it does not mean
   later English edits were automatically translated.
4. Keep the language selector links absolute so they work from both GitHub and
   crates.io. The English README links to headings by their automatic anchors
   (`#quick-start`); a translation has different heading text, so it declares an explicit
   `<a id="quick-start"></a>` before each linked heading. Relative file and image links
   resolve from this directory, one level below the repository root (`../docs/...`).
5. Run the focused documentation check:

   ```sh
   python3 scripts/check_readme_translations.py
   ```

The checker catches changed examples, missing sections, missing language links, broken
local links and changed external URLs. Keep GitHub alerts such as `> [!NOTE]` outside
HTML containers: GitHub renders nested alerts as literal text. It does not judge
translation quality. Review the prose for natural phrasing and technical accuracy before
opening a pull request.

For Arabic, preserve readable right-to-left prose and left-to-right code, paths and
identifiers. Use HTML direction attributes when needed, without changing the contents of
fenced blocks.

Public documentation stays free of private infrastructure details: no host names,
addresses, home directories or drive models, in any language.

New languages are welcome. Add a complete `README.<locale>.md`, include it in the
selector in English and every translation, and update `LOCALES` in
[the checker](../scripts/check_readme_translations.py).
