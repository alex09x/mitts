# Running tests in parallel processes: `-c` / `--concurrency`

```sh
mitts test -c                 # two thirds of the cores
mitts test -c=8               # 8 processes at once
mitts test --concurrency=8 quick_   # same, with a name filter
```

By default `mitts test` runs the test binaries one after another and the tests inside a
binary on threads. With `-c` every test runs in a process of its own, the way cargo-nextest
does it, and the tests of all binaries share one pool:

- each test binary lists its tests (`--list`), then every test runs as `binary --exact NAME`;
- a test cannot affect another through shared state, and a crash or a hang loses one test;
- the pool is two thirds of the cores (at least one) unless a number is given; there is no
  configuration file;
- the output of a test is captured and printed when it fails, with the test name;
- a failure stops starting new tests unless `--no-fail-fast` is given; the exit code is 101;
- `TESTNAME` and `--exact` select tests as usual; `#[ignore]` tests are skipped, `--ignored`
  runs only them.

Write the number with `=` (`-c=8`, `--concurrency=8`): a bare `-c` leaves the next word to the
test filter, so `mitts test -c quick` filters on `quick`. `-j` is a different option: it limits
concurrent build units.

Limits: a binary without the standard harness (`harness = false`) is run once as a whole;
arguments after `--` are not passed to the test processes; documentation tests run after the
tests, as usual; there are no retries, per-test timeouts or JUnit output yet.
