<!-- translation-source: 1a926080b49fae712d44fdae44fb6f96b80d3499 -->

> [!NOTE]
> هذه ترجمة عربية للتوثيق الرسمي باللغة الإنجليزية عند الإيداع [1a92608](https://github.com/alex09x/mitts/blob/1a926080b49fae712d44fdae44fb6f96b80d3499/README.md). النسخة المعتمدة والأحدث هي [README باللغة الإنجليزية](https://github.com/alex09x/mitts/blob/main/README.md).

<p align="center">
  <img src="../site/logo.svg" width="96" height="96" alt="Mitts Build logo">
</p>

<h1 align="center">Mitts Build</h1>

<div dir="rtl">

<p align="center">
  <b>عمل البناء، محسوب بالكامل.</b><br>
  نظام بناء يضع Rust في المقام الأول مع ذاكرة تخزين مؤقت للمخرجات معنونة بالمحتوى على نطاق الجهاز بأكمله وتتبع صريح للمدخلات. يقرأ ملفي <code>Cargo.toml</code> و <code>Cargo.lock</code> الحاليين لديك مباشرة.
</p>

</div>

<p align="center">
  <a href="https://mittsbuild.com"><img src="https://img.shields.io/badge/site-mittsbuild.com-ff3b30" alt="Website"></a>
  <img src="https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-c9ced4" alt="License: MIT OR Apache-2.0">
  <img src="https://img.shields.io/badge/language-Rust-1b1d20?logo=rust" alt="Written in Rust">
  <img src="https://img.shields.io/badge/platform-Linux%20%7C%20macOS-1b1d20" alt="Platforms: Linux, macOS">
  <img src="https://img.shields.io/badge/status-pre--release-ff3b30" alt="Status: pre-release">
  <a href="https://doi.org/10.5281/zenodo.24030067"><img src="https://zenodo.org/badge/DOI/10.5281/zenodo.24030067.svg" alt="DOI: 10.5281/zenodo.24030067"></a>
</p>

<p align="center" dir="rtl">
  <a href="#quick-start">البدء السريع</a> ·
  <a href="#how-it-works">كيف يعمل</a> ·
  <a href="#architecture">البنية المعمارية</a> ·
  <a href="../docs/architecture.md">البروتوكول</a> ·
  <a href="#limitations">القيود</a> ·
  <a href="../ROADMAP.md">خارطة الطريق</a>
</p>

<p align="center">
  <a href="https://github.com/alex09x/mitts/blob/main/README.md">English</a> ·
  <a href="https://github.com/alex09x/mitts/blob/main/readme/README.ru.md">Русский</a> ·
  <a href="https://github.com/alex09x/mitts/blob/main/readme/README.zh-CN.md">简体中文</a> ·
  <a href="https://github.com/alex09x/mitts/blob/main/readme/README.es.md">Español</a> ·
  <a href="https://github.com/alex09x/mitts/blob/main/readme/README.pt-BR.md">Português (Brasil)</a> ·
  <a href="https://github.com/alex09x/mitts/blob/main/readme/README.ja.md">日本語</a> ·
  <a href="https://github.com/alex09x/mitts/blob/main/readme/README.de.md">Deutsch</a> ·
  <a href="https://github.com/alex09x/mitts/blob/main/readme/README.fr.md">Français</a> ·
  <a href="https://github.com/alex09x/mitts/blob/main/readme/README.ko.md">한국어</a> ·
  <a href="https://github.com/alex09x/mitts/blob/main/readme/README.ar.md"><b>العربية</b></a>
</p>

---

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="../docs/img/bench-hero-dark.svg">
    <source media="(prefers-color-scheme: light)" srcset="../docs/img/bench-hero-light.svg">
    <img src="../docs/img/bench-hero-light.svg" width="920" alt="بناء 12 شجرة عمل لـ 100 مشروع Rust على جهاز Mac يستغرق 13 دقيقة مع Mitts و76 دقيقة مع Cargo؛ وتنخفض تشغيلات المترجم من 90,961 إلى 6,905 والكتابة على القرص من 641 GB إلى 112 GB">
  </picture>
</p>

<p align="center" dir="rtl">
  <sub>100 مشروع مفتوح المصدر (90 على Linux x86_64)، 12 شجرة عمل لكل مشروع، تُبنى معاً في آنٍ واحد على ثلاثة أجهزة. Cargo وCargo + sccache كخطي أساس، شاملةً الحالات التي لا يكون فيها Mitts أسرع: <a href="#performance">الأداء</a> · <a href="https://mittsbuild.com/the-hundred/"><bdi>The Hundred</bdi></a></sub>
</p>

<div dir="rtl">

يُخطط Mitts لمساحة العمل بنفسه ويستدعي `rustc` مباشرة. يحصل كل إجراء ترجمة على مفتاح مشتق من جميع مدخلاته. ويتم تخزين المخرجات مرة واحدة في مستودع غير قابل للتغيير على مستوى الجهاز بالكامل، وتُستعاد إلى كل مساحة عمل تحتاج إليها، عبر المشاريع وشجرات العمل والفروع.

إذا تعذر حساب أحد المدخلات، يُعاد بناء الإجراء دون تخمين.

<a id="the-problem"></a>
## المشكلة

تستقر الذاكرة المؤقتة لبناء Cargo داخل دليل <bdi>`target/`</bdi> الخاص بكل مساحة عمل. وأي عملية تنشئ <bdi>`target/`</bdi> جديداً تبدأ من الصفر:

- استنساخ ثانٍ أو `git worktree` لنفس المستودع،
- الانتقال إلى فرع يغير ملف `Cargo.lock` ثم العودة مرة أخرى،
- مشروع آخر يعتمد على نفس الحزم بنفس الميزات،
- وكلاء البرمجة المتوازيون، حيث يعمل كل منهم في شجرة عمل خاصة به.

يُعاد تجميع نفس الحزم مثل `serde` أو `syn` أو `tokio` مراراً وتكراراً، وتستهلك كل نسخة مساحة على القرص.

الحلول الالتفافية المعتادة تنطوي على عيوب حادة:

- **مشاركة `CARGO_TARGET_DIR`** تجعل عمليات البناء متسلسلة خلف قفل واحد، وتقوم المشاريع ذات مجموعات الميزات المختلفة بالكتابة فوق مخرجات بعضها البعض.
- **مغلف المترجم مثل sccache** يرى استدعاءً واحداً لـ `rustc` في كل مرة. ويعتمد مفتاحه على مسارات مطلقة ما لم يتم توحيدها، كما أن الحزم التي تستدعي الرابط (`bin` و `proc-macro` و `dylib`) لا يتم حفظها في الذاكرة المؤقتة. ويظل Cargo يخطط للبناء ويشغل كل سكربت بناء.

يسلك Mitts مساراً مختلفاً: فهو يحل محل منفّذ Cargo، وليس المترجم. إنه يرى مخطط الوحدات (unit graph) بأكمله، لذا يمكنه اشتقاق مفتاح كل وحدة بناءً على مفاتيح تبعياتها، وتخزين وحدات الربط وعمليات تشغيل سكربتات البناء مؤقتاً، وتمكين عدة عمليات بناء من مشاركة مستودع واحد بأمان تام.

<a id="highlights"></a>
## أبرز المميزات

- **إعادة الاستخدام عبر مساحات العمل.** مستودع واحد لكل جهاز. التبعية التي جُمعت لنسخة عمل (checkout) معينة تُستعاد إلى نسخة العمل التالية بنفس المدخلات.
- **مفاتيح مكتملة.** سلسلة الأدوات، والهدف، والملف التعريفي، والخيارات، والميزات، ومحتويات المصدر، ومفاتيح التبعيات، والبيئة المتتبعة. أي مدخلات غير معروفة تعني إعادة البناء.
- **تبقى ملفات بيان Cargo هي العقد المرجعي.** لا حاجة لملف بيان جديد ولا ملف قفل ثانٍ. تُنفذ تدفقات عمل البناء والاختبار والفحص والتغطية واستخراج حزم المصدر للإصدار في هذا المستودع عبر Mitts؛ ولا تستدعي واجهة أوامر Cargo. ويبقى التوافق الخارجي مع Cargo متاحاً للمستخدمين.
- **تخزين وحدات الربط وسكربتات البناء مؤقتاً.** الملفات الثنائية والاختبارات ووحدات الماكرو الإجرائية (proc macros) وعمليات تشغيل سكربتات البناء يتم اشتقاق مفاتيح لها وتخزينها، وليس فقط مكتبات `rlib`.
- **تعاون عمليات البناء المتزامنة.** أقفال لكل مفتاح: إذا احتاجت نافذتا أوامر أو شجرتا عمل نفس التبعية، فإنهما تقومان بتجميعها مرة واحدة. يحدد خادم مهام واحد سقف التوازي لجميع عمليات Mitts على الجهاز.
- **مشاركة آمنة.** عناصر المستودع غير قابلة للتغيير: تُنشر ذرياً، وتُخزن للقراءة فقط، وتُستعاد عبر reflink أو النسخ. يعمل `MITTS_VERIFY_RESTORE=1` على إعادة حساب التجزئة لكل عنصر قبل استعادته (الافتراضي يثق بالمستودع، مما يوفر قراءة واحدة لكل بايت مستعاد). وحيث لا يدعم نظام الملفات تقنية reflink، تستعيد العملية العنصر عبر ربطه برابط ثابت للقراءة فقط من المستودع إلى <bdi>`target/`</bdi> بدلاً من نسخ بايتاته: يحتفظ الـ inode المشترك بالوضع `0o444` (و `0o555` للملفات التنفيذية)، ويرفض فهرس الهدف أي رابط يكتسب بتات كتابة، ويستبدل Mitts ملفات مساحة العمل فقط عن طريق إعادة التسمية. لا يوجد رابط قابل للكتابة يشير إلى عنصر في المستودع مطلقاً. يحتفظ `MITTS_STORE_HARDLINKS=0` بنسخ خاصة لمساحات العمل التي تكتب فيها الأدوات الأخرى في المكان مباشرة.
- **تدريج مساحة العمل دون نسخ للبيانات.** تستخدم الأسماء المستعارة للمكتبات داخل مساحة العمل والملفات الثنائية الثانوية والتدريج الذري للوحدات روابط ثابتة داخل مساحة العمل وترقية عن طريق إعادة التسمية دون نسخ أي بايتات. عندما يقع `MITTS_CACHE_DIR` و <bdi>`target/`</bdi> الخاص بمساحة العمل على نفس وحدة تخزين نظام الملفات، تعمل ترقيات التدريج دون نسخ للبايتات؛ وعبر وحدات التثبيت المنفصلة، يتم الرجوع بسلاسة وأمان إلى النسخ القياسي.
- **المسار السريع للعمليات غير المتغيرة.** تتخطى مساحة العمل التي لم تتغير مراحل التخطيط واشتقاق المفاتيح بالكامل.
- **واجهة أوامر موحدة.** تشمل `build` و `run` و `check` و `clippy` و `fmt` و `test` (بما في ذلك اختبارات التوثيق doctests) و `coverage` و `explain` و `fetch` و `gc` و `clean`.

<a id="quick-start"></a>
## البدء السريع

ثبّت أحدث إصدار أصيل موقّع:

<div dir="ltr">

```sh
curl -fsSL https://mittsbuild.com/install.sh | sh
```

</div>

لبناء النسخة من المصدر، ثبّت أولاً أحدث إصدار منشور من Mitts، ثم استخدم Mitts لبناء النسخة المستنسخة:

<div dir="ltr">

```sh
git clone https://github.com/alex09x/mitts
cd mitts
mitts build --release
MITTS_JOBS=2 mitts build     # limit concurrent native build units
install -m755 target/release/mitts "$HOME/.cargo/bin/mitts"
```

</div>

ثم قم بتشغيله داخل أي مساحة عمل لـ Cargo:

<div dir="ltr">

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

</div>

بالنسبة لمشاريع Git، تتطلب عملية تحزيم الحزم حالياً مستودعاً نظيفاً مع وجود الحزمة في جذره. يمكن لأنماط `include` في ملف البيان تحديد الملفات المتجاهلة؛ ولا يتم دعم حزم مساحة العمل المتداخلة والدلائل ذات الروابط الرمزية حتى الآن. يستهدف الأمر `mitts publish` موقع crates.io ويقرأ الرمز المميز من `CARGO_REGISTRY_TOKEN`.

يكتب Mitts مخرجات البناء في أدلة Cargo المعتادة <bdi>`target/debug/`</bdi> و <bdi>`target/release/`</bdi>. يحدد `CARGO_TARGET_DIR` و <bdi>`--target-dir`</bdi> جذراً مختلفاً للمخرجات. ذاكرته المؤقتة المشتركة منفصلة (<bdi>`--cache-dir`</bdi>، وإلا <bdi>`$MITTS_CACHE_DIR`</bdi>، وإلا <bdi>`$XDG_CACHE_HOME/mitts`</bdi>، وإلا <bdi>`~/.cache/mitts`</bdi>). ويستقر مستودع المخرجات، والذاكرة المؤقتة للإجراءات، والأقفال، وخادم المهام، ومستودع المصادر، ومرايا git، وفهرس الذاكرة المؤقتة تحت ذلك الجذر الموحد.

يتم استنساخ تبعيات Git، العامة منها أو الخاصة، بواسطة Mitts نفسه باستخدام `git` المثبت على النظام، بحيث تنطبق مفاتيح SSH والوكيل ومساعدو الاعتماد و `GIT_SSH_COMMAND` تماماً كما في سطر الأوامر؛ ويتم تعطيل المطالبات التفاعلية، وبالتالي يفشل الإجراء فوراً عند غياب بيانات الاعتماد بدلاً من التعليق. لا يمر أي شيء عبر Cargo. ويُعاد استخدام أي نسخة عمل أنجزها Cargo مسبقاً تحت <bdi>`~/.cargo/git/checkouts`</bdi> بدلاً من استنساخه مجدداً.

وتعمل جميع الطرق الأخرى التي يعتمدها Cargo لجلب الحزم بنفس الطريقة ومن نفس الإعدادات:

- **السجلات البديلة**: <bdi>`[registries.<name>] index = "sparse+https://…"`</bdi> مع <bdi>`registry = "<name>"`</bdi> في التبعية. تأتي الرموز المميزة من موفري اعتمادات Cargo، وفق ترتيب Cargo: `cargo:token` (البيئة و `credentials.toml`)، و `cargo:macos-keychain`، و `cargo:libsecret`، و <bdi>`cargo:token-from-stdout <command>`</bdi>، والإضافات مثل `cargo-credential-1password` عبر بروتوكول موفر الاعتمادات، والمعدة بواسطة <bdi>`[registry] global-credential-providers`</bdi> و <bdi>`[registries.<name>] credential-provider`</bdi> و <bdi>`[credential-alias]`</bdi>. إذا نص ملف `config.json` للسجل على `auth-required`، تفشل العملية برسالة واضحة عند عدم توفر رمز مميز لدى أي موفر. تُرفض السجلات ذات فهارس git: يُرجى استخدام بروتوكول sparse.
- **المرايا**: استخدام <bdi>`[source.crates-io] replace-with = "<name>"`</bdi> مع <bdi>`[source.<name>] registry = "sparse+https://…"`</bdi>. تأتي البيانات من المرآة، بينما يواصل `Cargo.lock` الإشارة إلى crates.io تماماً كما يكتبه Cargo.
- **المصادر الموردة**: <bdi>`[source.vendored-sources] directory = "vendor"`</bdi> الناتجة عن `cargo vendor`. تُفحص الحزم مقابل ملف <bdi>`.cargo-checksum.json`</bdi> الخاص بها وتُنسخ إلى المستودع؛ دون الاتصال بالشبكة على الإطلاق، بما في ذلك حزم git. في حال غياب ملف القفل، يعمل دليل vendor كفهرس.
- **<bdi>`[patch]`</bdi>**: استبدال حزمة السجل عبر <bdi>`[patch.crates-io] foo = { path = … }`</bdi> أو <bdi>`{ git = … }`</bdi>؛ وفي غياب ملف القفل، يتم حل التعيين وتثبيته، ويجب أن يفي بمتطلبات الإصدار، تماماً كما هو الحال في Cargo.

يتم تطبيق قسم <bdi>`[env]`</bdi> في إعدادات Cargo على rustc وسكربتات البناء والبرامج التي يشغلها `mitts run` و `mitts test`، وفقاً لقواعد `force` و `relative` المتبعة في Cargo؛ وتعد ملفات التكوين مدخلات أساسية، وبالتالي فإن أي تغيير في القيمة يفرض إعادة بناء ما قرأها.

وفي حال عدم وجود ملف `Cargo.lock`، يقوم Mitts بحل التبعيات بنفس طريقة Cargo: اختيار أعلى إصدار غير ملغى لكل متطلب، مع سريان الميزات عبر حواف المخطط، وتضمين التبعيات الاختيارية فقط عند تفعيل الميزة لها، وإصدار واحد لكل نطاق توافق. وتكون النتيجة ملف قفل يقبله Cargo كملف خاص به تماماً.

<a id="how-it-works"></a>
## كيف يعمل

<div dir="ltr">

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

</div>

يتضمن مفتاح الوحدة **مفاتيح** تبعياتها، وليس بايتات مخرجاتها، لذا فإن أي تغيير في أي مستوى أسفل الوحدة يغير مفتاحها. تعمل سكربتات البناء في دليل تدريج مرحلي؛ وتدخل توجيهاتها (`rustc-cfg` و <bdi>`rustc-link-*`</bdi> و `rustc-env` وغيرها) في مفتاح الوحدة التي تقوم بتهيئتها، بينما تحدد إعلانات <bdi>`rerun-if-*`</bdi> الخاصة بها متى يمكن إعادة استخدام عملية التشغيل المخزنة مؤقتاً.

بعد نجاح عملية البناء، يسجل Mitts لقطة لمساحة العمل. فإذا لم يتغير شيء فيها، تتخطى عملية البناء التالية المراحل مباشرة إلى النتيجة. وأي عدم تطابق يؤدي إلى العودة للبناء الكامل المستند إلى المفاتيح، وبالتالي فإن المسار السريع لا يتجاوز أبداً أي عمل يقتضيه البروتوكول.

البروتوكول الكامل — اشتقاق المفاتيح، والأقفال، وسكربتات البناء، والمسار السريع، وتخطيط المستودع، وجمع المهملات — موضح في [docs/architecture.md](../docs/architecture.md).

<a id="architecture"></a>
## البنية المعمارية

يتألف Mitts من نواة محايدة للغات يعلوها محولات خاصة بكل لغة. لا تعرف النواة شيئاً عن Cargo؛ بينما يقوم محول Rust بتحويل مساحة عمل Cargo إلى إجراءات ذات مفاتيح.

<div dir="ltr">

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

</div>

| المسار | المحتويات |
| --- | --- |
| <bdi>`src/cli/`</bdi> | معالجات الأوامر: `build` و `test` و `clippy` و `explain` وغيرها |
| <bdi>`src/adapter/rust/`</bdi> | دلالات Cargo: ملفات البيان، وملف القفل، وفهرس sparse، وحل الميزات، وتخطيط الوحدات، ومفاتيح الإجراءات، ومنفذ DAG، وسكربتات البناء، و doctests |
| <bdi>`src/adapter/c/`</bdi> | محول تجريبي لـ C/C++ يعتمد على نفس المستودع |
| <bdi>`src/core/`</bdi> | مستودع محايد للغات (`cas.rs`)، وذاكرة الإجراءات المؤقتة، والأقفال، وخادم المهام، والمسار السريع (`build_lock*.rs`)، وجمع المهملات (GC)، و reflinks |
| <bdi>`docs/`</bdi> | [البروتوكول](../docs/architecture.md)، و[قرارات التصميم](../docs/decisions)، و[السيناريوهات](../docs/scenarios) |
| <bdi>`tests/`</bdi> | اختبارات التكامل، بما في ذلك التكافؤ مع Cargo واختبارات إبطال الذاكرة المؤقتة |
| <bdi>`scripts/`</bdi> | أدوات قياس الأداء والتدقيق |
| <bdi>`fixtures/`</bdi> | مشاريع مصغرة لـ Rust و C مستخدمة في الاختبارات |

<a id="what-invalidates-the-cache"></a>
## ما الذي يُبطل الذاكرة المؤقتة

يتطلب تحقيق إصابة للذاكرة المؤقتة تطابق جميع المدخلات الموضحة أدناه. فإذا تعذر على Mitts التحقق من مدخل ما، فإنه لا يعيد استخدام النتيجة.

| المدخل | طريقة التتبع |
| --- | --- |
| سلسلة الأدوات | مخرجات `rustc -vV` |
| الهدف والملف التعريفي | ثلاثية الهدف، وإعدادات الملف التعريفي، و `RUSTFLAGS` وخيارات rustflags في إعدادات Cargo |
| الميزات | مجموعة الميزات المحلولة للوحدة (عبر resolver 1 أو 2، تماماً كما يفعل Cargo) |
| المصادر | تجزئات محتوى ملفات الحزمة وكل ملف يفيد rustc بقراءته (<bdi>`include_str!`</bdi> و <bdi>`include_bytes!`</bdi> والوحدات النمطية) |
| التبعيات | مفاتيح الإجراءات لجميع وحدات التبعيات |
| البيئة | إعدادات <bdi>`CARGO_*`</bdi> و <bdi>`RUST_*`</bdi> ذات الصلة، والمتغيرات المقروءة عبر <bdi>`env!`</bdi> / <bdi>`option_env!`</bdi>، مع تسجيل <bdi>`<unset>`</bdi> صراحة للمتغير غير المحدد |
| إعدادات Cargo | ملفات <bdi>`.cargo/config(.toml)`</bdi> التي يقرأها Cargo |
| سكربتات البناء | توجيهاتها، و `OUT_DIR`، ومحتوى مسارات `rerun-if-changed` وقيم متغيرات `rerun-if-env-changed` |

يتم استدعاء برامج `rustc-wrapper` و `rustc-workspace-wrapper` التي تم تكوينها لعملية الترجمة. ونظراً لأن أي مغلف اعتباطي يمكن أن يعتمد على مدخلات غير متتبعة، يعطل Mitts إعادة استخدام ذاكرة الإجراءات المؤقتة وذاكرة سكربتات البناء المؤقتة أثناء نشاط أي من المغلفين. قد يؤدي ذلك إلى إبطاء عمليات البناء تلك.

تقرأ بعض وحدات الماكرو الإجرائية (proc macros) ملفات من حزمتك أثناء توسعها: إذ يفحص `sqlx` الاستعلامات مقابل <bdi>`.sqlx/`</bdi>، ويجمع `askama` قوالب <bdi>`templates/`</bdi>، ويقوم `rust-embed` و `include_dir` بتضمين دلائل كاملة. لا يعلم rustc بمثل هذه القراءات إلا إذا سجلها الماكرو صراحة، لذا ينبغي الإعلان عن هذه الحزم في ملف `Cargo.toml` الرئيسي:

<div dir="ltr">

```toml
[workspace.metadata.mitts]          # [package.metadata.mitts] in a single crate
impure-proc-macros = ["sqlx", "askama", "rust-embed"]
```

</div>

الحزم التي تعتمد على ماكرو مدرج يتم اشتقاق مفاتيحها بناءً على كل ملف في حزمتها وعلى مسارها المطلق: يؤدي أي تعديل في القالب إلى إعادة بنائها، ولا تعيد أي نسخة عمل أخرى استخدامها أبداً. وتُعامل جميع وحدات الماكرو الأخرى على أنها نقية، وتتم مشاركة الوحدات المعتمدة عليها عبر شجرات العمل. يكفي ذكر اسم الحزمة التي تعتمد عليها؛ وتعمل حزمة الماكرو التي تجلبها (`sqlx-macros`) أيضاً.

<a id="comparison"></a>
## المقارنة

مقارنة نوعية لكيفية إعادة استخدام العمل المنجز في كل أداة. وهي ليست ادعاءً في سرعة الأداء.

| | Cargo | Cargo + sccache | Mitts |
| --- | --- | --- | --- |
| قراءة `Cargo.toml` / `Cargo.lock` كما هما | نعم | نعم | نعم |
| إعادة الاستخدام عبر مساحات العمل وشجرات العمل | لا (لكل <bdi>`target/`</bdi>) | نعم، لكل استدعاء لـ `rustc` | نعم، لكل وحدة |
| اعتماد المفتاح على مسار نسخة العمل | — | نعم، ما لم يتم توحيده | لا لحزم السجلات؛ ولحزم مساحة العمل فقط عند عدم الأهلية |
| تخزين وحدات `bin` والاختبارات و `proc-macro` مؤقتاً | — | لا | نعم |
| تخزين تشغيل سكربتات البناء مؤقتاً | لا | لا | نعم |
| عمليات بناء متزامنة لنفس التبعية | انتظار قفل <bdi>`target/`</bdi> | قد يقوم كلاهما بالبناء عند الإخفاق | يقوم أحدهما بالبناء والآخر يعيد الاستخدام |
| ذاكرة مؤقتة بعيدة / للفريق | لا | نعم (S3, GCS, Redis, …) | مخطط له |
| نظام Windows | نعم | نعم | لا |

يحل كل من Bazel و Buck2 مشكلة أوسع نطاقاً — عمليات بناء محكمة ومتعددة اللغات مع تنفيذ عن بُعد — ولكنهما يتطلبان ملفات بناء خاصة بهما بدلاً من الاعتماد على `Cargo.toml`.

<a id="limitations"></a>
## القيود

لا يزال Mitts في مرحلة ما قبل الإصدار. يرجى الانتباه للنقاط التالية قبل الاعتماد عليه:

- **Linux و macOS فقط.** تعتمد الأقفال وخادم المهام على معايير POSIX.
- **الهدف المضيف فقط.** الترجمة المتقاطعة غير مدعومة حتى الآن.
- **لا توجد ذاكرة مؤقتة عن بُعد بعد.** المستودع محلي لكل جهاز.
- **سجلات sparse فقط.** تُرفض السجلات التي تعمل عبر فهرس git (أي <bdi>`registry+…`</bdi> بخلاف crates.io، أو <bdi>`[source] registry = "https://…"`</bdi> بدون <bdi>`sparse+`</bdi>) ومصادر `local-registry` مع تقديم نصائح بديلة.
- **بيانات الاعتماد تُقرأ فقط ولا تُكتب أبداً.** تبقى مهمة `cargo login` موكلة إلى Cargo؛ ويتم تجاهل `cargo:wincred` لكونه خاصاً بنظام Windows فقط.
- **بعض الوحدات لا تُخزن مؤقتاً أبداً:** الحزم من الأنواع `cdylib` و `dylib` و `staticlib`، والحزم التي يتعذر تتبع قراءاتها لـ <bdi>`env!`</bdi>. يتم بناؤها دائماً؛ ويبلغ عنها الأمر `mitts explain`.
- **حزم مساحات العمل عبر نسخ العمل.** يُعاد استخدام حزمك الخاصة عبر شجرات العمل فقط عندما يكون مفتاحها مستقلاً عن المسار (عدم وجود قراءات لـ `CARGO_MANIFEST_DIR`، وعدم وجود سكربت بناء، وعدم وجود ماكرو إجرائي غير نقي مُعلن عنه). وإلا فإن المفتاح سيتضمن المسار المطلق. على نظام macOS، تُكتب علامات تصحيح الأخطاء الخاصة بالرابط نسبة إلى مساحة العمل (<bdi>`-oso_prefix`</bdi>)، مما يسمح بمشاركة الملفات الثنائية المرتبطة أيضاً؛ ابدأ المصحح في جذر مساحة العمل ليتمكن من العثور على ملفات الكائنات.
- **وحدات الماكرو التي تقرأ ملفاتك تقع مسؤولية إعلانها عليك.** يُعامل الماكرو الإجرائي كـ نقي ما لم يدرجه المشروع في القائمة (انظر أعلاه). تتبع الوحدات المعتمدة على الماكرو المدرج كل ملف في حزمتها؛ أما الماكرو غير المدرج الذي يقرأ ملفاً دون تسجيله لدى rustc فيمكن أن ينتج نتيجة قديمة، تماماً كما هو الحال في Cargo.
- **سكربتات البناء لا تعمل في بيئة معزولة (sandbox).** السكربت الذي يقرأ ملفات أو متغيرات لم يعلن عنها عبر <bdi>`rerun-if-*`</bdi> يمكن أن ينتج نتيجة قديمة — تماماً كما هو الحال في Cargo.
- **ميزات Cargo غير المدعومة تفشل بصوت عالٍ.** على سبيل المثال خيار <bdi>`--profile`</bdi>، أو الأنماط العمومية (globs) ذات الحروف البديلة في الدلائل الوسيطة داخل `workspace.members`. يبلغ Mitts عنها بدلاً من التخمين.
- **عمليات النسخ الصفري على نفس نظام الملفات.** تتطلب ترقية الوحدات دون نسخ للبيانات من مرحلة التدريج أن يستقر `MITTS_CACHE_DIR` ودليل <bdi>`target/`</bdi> لمساحة العمل على نفس وحدة التثبيت لنظام الملفات. والعبور بين حدود أنظمة الملفات يتراجع بأمان إلى النسخ القياسي.

**لا تستخدم Mitts حتى الآن** إذا كنت بحاجة إلى نظام Windows، أو الترجمة المتقاطعة، أو ذاكرة مؤقتة للفريق، أو إصدار مستقر. يظل Cargo، مع sccache اختيارياً، هو الخيار الآمن في تلك الحالات.

<a id="performance"></a>
## الأداء

قِيسَ الأداء باستخدام Mitts <bdi>`0.3.17`</bdi> عبر **100 مشروع Rust مفتوح المصدر** على macOS (Apple M2 Ultra، بنظام APFS) وLinux ARM64 (بنظام XFS)، و**90 مشروعاً** على Linux x86_64 (AMD Ryzen 9 9950X، بنظام XFS)، في مقابل `cargo` العادي و`cargo` مع `sccache` (<bdi>`0.17.0`</bdi>). تمثل كل قراءة الوسيط لـ 3 تكرارات مع التحقق من تطابق الرسائل التشخيصية ورموز الخروج ونتائج الاختبارات؛ وتبقى جميع المشاريع المقيسة ضمن الإجماليات (<bdi>`0`</bdi> و<bdi>`7`</bdi> و<bdi>`0`</bdi> تشغيلات موسومة بالضوضاء)، ولا يتجاوز أي رقم القيمة المقيسة: إذ تُقتطع قيم خط الأساس إلى الأدنى وتُقرَّب قيم Mitts إلى الأعلى ([ROADMAP M8](../ROADMAP.md)). الدراسة الكاملة وكل مشروع والمنهجية متاحة على [mittsbuild.com/the-hundred](https://mittsbuild.com/the-hundred/).

<div dir="ltr" align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="../docs/img/bench-scenarios-dark.svg">
    <source media="(prefers-color-scheme: light)" srcset="../docs/img/bench-scenarios-light.svg">
    <img src="../docs/img/bench-scenarios-light.svg" width="920" alt="زمن بناء المشروع الوسيط عبر ستة سيناريوهات: Cargo وCargo + sccache وMitts">
  </picture>
</div>

يمثل كل صف زمن بناء المشروع الوسيط (الوسيط الخاص بكل أداة عبر المشاريع، بمقياس خطي لكل صف)، وبجانبه عدد المشاريع التي أنهاها Mitts أولاً. على macOS يبني المشروع الوسيط شجرات عمله الـ 12 في <bdi>**32 s**</bdi> مع `cargo`، و<bdi>**31 s**</bdi> مع `cargo` + `sccache`، و<bdi>**3.8 s**</bdi> مع Mitts. ومع مستودع محلي دافئ، تستغرق شجرة عمل ثانية نظيفة <bdi>**1.7 s**</bdi> و<bdi>**1.6 s**</bdi> و<bdi>**79 ms**</bdi>؛ وتبديل الفرع <bdi>**627 ms**</bdi> و<bdi>**209 ms**</bdi> و<bdi>**27 ms**</bdi>؛ وبناء بلا تغييرات <bdi>**100 ms**</bdi> و<bdi>**41 ms**</bdi> و<bdi>**9 ms**</bdi>. ويحل Mitts أولاً في <bdi>99–100</bdi> من أصل 100 مشروع في تلك السيناريوهات الأربعة (71 من 71 لشجرة العمل الثانية).

الحالات التي لا يتفوق فيها Mitts:

- **بناء بارد بذاكرة مؤقتة فارغة.** ينتهي `cargo` العادي أولاً في 71 من أصل 100 مشروع على macOS (60 من 90 على Linux x86_64، و77 من 100 على Linux aarch64)، بفارق يصل إلى <bdi>6.2 s</bdi> في أكبر المشاريع، لأن Mitts يحسب تجزئات المدخلات ويملأ مستودعاً فارغاً. يستغرق المشروع الوسيط <bdi>**1.7 s**</bdi> مع `cargo` و<bdi>**2.0 s**</bdi> مع Mitts. ويتفوق Mitts على `sccache` البارد (<bdi>**2.4 s**</bdi>، وMitts أولاً في 86 من 100).
- **تعديل ملف واحد.** يستغرق المشروع الوسيط <bdi>**364 ms**</bdi> مع `cargo`، و<bdi>**596 ms**</bdi> مع `sccache`، و<bdi>**315 ms**</bdi> مع Mitts، ويحل Mitts أولاً في 49 من أصل 71 مشروعاً مقابل `cargo`. بينما يحل `cargo` أولاً في المشاريع الـ 22 الأخرى (21 من 64 على x86_64، و29 من 71 على aarch64)، بفارق يصل إلى <bdi>2.6 s</bdi> على macOS و<bdi>8.6 s</bdi> على x86_64، لأنه يعيد استخدام الترجمة التزايدية لـ `rustc` داخل الحزمة الكبيرة، في حين يخزن Mitts وحدات الحزم كاملةً.
- **إعادة تشغيل 10 إيداعات** (غير معروضة في الرسم): النتيجة منقسمة، إذ يحل Mitts أولاً في 58 من أصل 100 مشروع على macOS مع تقارب الوسائط (<bdi>**5.6 s**</bdi> مع `cargo`، و<bdi>**5.8 s**</bdi> مع Mitts).

<div dir="ltr" align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="../docs/img/bench-swarm-dark.svg">
    <source media="(prefers-color-scheme: light)" srcset="../docs/img/bench-swarm-light.svg">
    <img src="../docs/img/bench-swarm-light.svg" width="920" alt="لوحة نتائج بناء 12 شجرة عمل متوازية عبر 100 مشروع Rust: الزمن والمعالج وتشغيلات المترجم والكتابة على القرص">
  </picture>
</div>

بناء **12 شجرة عمل متوازية** عبر 4 إيداعات في جميع المشاريع، الإجماليات لكل جهاز:

| الجهاز | الزمن: `cargo` / `sccache` / Mitts | تشغيلات `rustc`: `cargo` / Mitts | الكتابة على القرص: `cargo` / Mitts |
|---|---|---|---|
| macOS, APFS, M2 Ultra (100 مشروع) | <bdi>76 min / 73 min / **13 min**</bdi> | <bdi>90,961 / **6,905**</bdi> | <bdi>641 GB / **112 GB**</bdi> |
| Linux x86_64, XFS, Ryzen 9 9950X (90 مشروعاً) | <bdi>60 min / 52 min / **9 min**</bdi> | <bdi>75,049 / **6,162**</bdi> | <bdi>651 GB / **106 GB**</bdi> |
| Linux aarch64, XFS (100 مشروع) | <bdi>78 min / 76 min / **22 min**</bdi> | <bdi>90,544 / **7,348**</bdi> | <bdi>743 GB / **121 GB**</bdi> |

ينخفض زمن المعالج (CPU) على macOS من <bdi>**20.3 h**</bdi> (`cargo`) و<bdi>**8.0 h**</bdi> (`sccache`) إلى <bdi>**2.7 h**</bdi>. قاس جهاز x86_64 ما مجموعه 90 مشروعاً لا 100: إذ غابت عشرة مشاريع عن ذلك التشغيل (<bdi>similar, smallvec, smol, syntect, tealdeer, tera, textwrap, tokei, tower-http, xh</bdi>)، ولم يُسجَّل السبب.


<a id="status"></a>
## الحالة

| المجال | الحالة |
| --- | --- |
| قراءة `Cargo.toml` / `Cargo.lock`، وحل التبعيات عبر فهرس sparse | مكتمل |
| حل الميزات (عبر resolver 1 و 2) | مكتمل |
| برنامج التنزيل المتوازي ومستودع المصادر العالمي | مكتمل |
| مشغل DAG الأصيل، وتشغيل `rustc` بخط أنابيب (pipelining)، وخادم المهام | مكتمل |
| مستودع المخرجات المعنون بالمحتوى والذاكرة المؤقتة للإجراءات | مكتمل |
| تنفيذ سكربتات البناء وتخزينها مؤقتاً | مكتمل |
| المسار السريع للعمليات غير المتغيرة | مكتمل |
| أوامر `check` و `clippy` و `fmt` و `test` (بما فيها doctests) و `coverage` | مكتمل |
| جمع المهملات التحفظي مع الحصص النسبية | مكتمل |
| محول C/C++ تجريبي على نفس المستودع | تجريبي |
| مفاتيح مستقلة عن المسار لحزم مساحة العمل | جزئي |
| دراسة قياس الأداء القابلة للتكرار | مكتمل |
| ذاكرة مؤقتة عن بُعد / للفريق | مخطط له |
| نظام Windows، والترجمة المتقاطعة | غير مدعوم |

المعالم الرئيسية والأدلة: [ROADMAP.md](../ROADMAP.md).

<a id="faq"></a>
## الأسئلة الشائعة

**هل يحل Mitts محل Cargo؟**
يقرأ Mitts ملفات بيان Cargo وملفات القفل الحالية ويوفر تدفق عمل البناء والاختبار لـ Rust في هذا المشروع. وتستخدم تدفقات عمل CI والإصدار الخاصة به Mitts دون استدعاء Cargo. ويبقى Cargo متاحاً للمستخدمين الخارجيين الذين يحتاجون إليه.

**هل يمكنني استخدامه في بيئات التكامل المستمر (CI)؟**
نعم. يثبت التكامل المستمر في هذا المستودع أحدث إصدار مستقر من Mitts، ثم يستخدمه لعمليات البناء الأصلية والاختبارات والتغطية وتنسيق الأكواد و Clippy. ويكون مستودع المخرجات المشترك محلياً لكل بيئة تشغيل؛ ولا تتوفر ذاكرة مؤقتة عن بُعد عبر الأجهزة حتى الآن.

**هل يتعارض مع rust-analyzer أو مخرجات Cargo الحالية؟**
يستخدم Mitts نفس مسارات المخرجات الافتراضية مثل Cargo. ويبقى مستودع المخرجات المشترك الخاص به منفصلاً، بينما يزيل الأمر `mitts clean` فقط مخرجات Mitts المفهرسة التي لا تزال تطابق محتواها المسجل؛ ويحافظ على ملفات Cargo غير المرتبطة أو المستبدلة. وإذا كان Cargo يحتفظ بقفل دليل البناء لملف تعريفي معين، يتم تخطي التنظيف حتى تنتهي عملية البناء تلك.

**أين تقع الذاكرة المؤقتة وكيف يمكنني مسحها؟**
عبر <bdi>`--cache-dir`</bdi>، وإلا <bdi>`$MITTS_CACHE_DIR`</bdi>، أو <bdi>`$XDG_CACHE_HOME/mitts`</bdi>، أو <bdi>`~/.cache/mitts`</bdi>؛ يستقر كل ما يخزنه Mitts تحت هذا الجذر الموحد. استخدم `mitts gc` للتقليم في حدود الحصص المحددة؛ كما يُعد حذف الدليل آمناً أيضاً عند عدم تشغيل أي عملية بناء.

**لماذا لم يتم تخزين وحدة معينة مؤقتاً؟**
شغّل الأمر `mitts explain`. سيقوم بسرد كل وحدة وتوضيح سبب أهليتها أو عدم أهليتها للتخزين المؤقت.

<a id="design-rules"></a>
## قواعد التصميم

- الحفاظ على حيادية تنسيق عمليات البناء والتخزين المؤقت تجاه اللغات. وتستقر الدلالات الخاصة باللغة في المحولات؛ وتعد Rust أولها.
- تتطلب إصابة الذاكرة المؤقتة مدخلات كاملة ومتتبعة. وتُعامل أي مدخلات مجهولة على أنها متغيرة.
- المخرجات المشتركة غير قابلة للتغيير ويتم التحقق منها قبل الاستخدام. وتبقى الحالة المتغيرة داخل كل مساحة عمل. لا روابط ثابتة لملفات قابلة للكتابة، ولا تلاعب بالطوابع الزمنية.
- لا تمس عمليات التنظيف مخرجات أي عملية بناء نشطة على الإطلاق.

قواعد المساهمين والوكلاء: [AGENTS.md](../AGENTS.md).
قرارات البنية المعمارية: [docs/decisions](../docs/decisions).

<a id="author"></a>
## المؤلف

<div dir="ltr">

[Alexander Panasenko](https://prod.codes/about/) · alex@prod.codes ·
[mittsbuild.com](https://mittsbuild.com)

</div>

<a id="citation"></a>
## الاستشهاد

للاستشهاد بـ Mitts، استخدم الإصدار المؤرشف. يشير معرّف DOI المفاهيمي <bdi>[10.5281/zenodo.24030067](https://doi.org/10.5281/zenodo.24030067)</bdi> دائماً إلى أحدث إصدار؛ ومعرّف DOI للإصدار 0.3.35 هو <bdi>[10.5281/zenodo.24147274](https://doi.org/10.5281/zenodo.24147274)</bdi>. تقرأ ميزة «Cite this repository» في GitHub الملف <bdi>[CITATION.cff](../CITATION.cff)</bdi>.

<div dir="ltr">

```bibtex
@software{panasenko_2026_24147274,
  author    = {Panasenko, Alexander},
  title     = {Mitts Build — Rust-first build system with a machine-wide, content-addressed artifact cache},
  year      = {2026},
  publisher = {Zenodo},
  version   = {0.3.35},
  doi       = {10.5281/zenodo.24147274},
  url       = {https://doi.org/10.5281/zenodo.24147274}
}
```

</div>

<a id="license"></a>
## الترخيص

مرخص بترخيص مزدوج بموجب [MIT](../LICENSE-MIT) أو [Apache-2.0](../LICENSE-APACHE)، وفقاً لاختيارك.

</div>
