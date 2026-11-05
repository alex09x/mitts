# Running tests in parallel processes: `-c` / `--concurrency`

```sh
mitts test -c                       # two thirds of the cores
mitts test -c=8                     # 8 processes at once
mitts test --concurrency=8 quick_   # same, with a name filter
mitts test -c --no-fail-fast        # run everything and list every failure
```

By default `mitts test` runs the test binaries one after another and the tests inside a
binary on threads. With `-c` every test runs in a process of its own, the way cargo-nextest
does it, and the tests of all binaries share one pool:

- each test binary lists its tests (`--list`), then every test runs as `binary --exact NAME`;
- a crash or a hang loses one test, and a test cannot change another one's memory, statics or
  environment variables;
- the pool is two thirds of the cores (at least one) unless a number is given; there is no
  configuration file;
- the output of a test is captured and printed when it fails, with the test name;
- a failure stops starting new tests unless `--no-fail-fast` is given; the exit code is 101;
- `TESTNAME` and `--exact` select tests as usual; `#[ignore]` tests are skipped, `--ignored`
  runs only them.

Write the number with `=` (`-c=8`, `--concurrency=8`): a bare `-c` leaves the next word to the
test filter, so `mitts test -c quick` filters on `quick`. `-j` is a different option: it limits
concurrent build units.

On this repository's own suite (945 tests, 244 binaries) on a 32-core Linux host: 95 s without
the flag, 18 s with `-c`.

## Dangers: tests that were never meant to run together

Separate processes remove the sharing *inside* a process. Everything *outside* a process is
still shared, and many tests at once will find it. A suite that passes with `mitts test` can
fail, rarely or always, with `-c`:

- **Fixed ports.** Two tests that bind `127.0.0.1:8080` collide. Only one gets it.
- **Fixed paths.** A file or directory with a constant name (`/tmp/test.db`, `./out.txt`, a
  directory inside the source tree or `target/`) is written, read and deleted by several tests
  at once.
- **Shared caches and homes.** `~/.cache`, `~/.cargo`, a global config or a package store
  that tests write to. Mitts's own tests give every build its own `--cache-dir` for this
  reason.
- **Shared services.** One database, one queue, one container, one `docker compose` project,
  the same table or topic name, a lock file.
- **In-process serialization that no longer works.** A `static Mutex`, `serial_test` or a
  test-thread count of 1 only serializes tests *inside one process*. With `-c` each test is its
  own process, so such a mutex does not protect an external resource any more.
- **Tests that depend on another test.** Order, or the state a previous test left behind
  (a file it created, a row it inserted). Each test now starts from nothing, in any order.
- **Timing.** A test that sleeps for a fixed time or asserts a duration can fail when the
  machine is busy with 20 other tests; heavy tests slow each other down.
- **Machine limits.** N processes use N times the memory, file descriptors and temp space of
  one; a test that is fine alone can hit an out-of-memory or "too many open files" error.

A conflict shows up as an intermittent failure that passes when the test runs alone. Do not
retry it away: it is a test that shares something it should own.

## Best practices

Using the flag:

1. Start with `mitts test -c --no-fail-fast`. Without it the first failure stops the run and
   hides the other conflicts.
2. Run the suite twice, and once with `-c=1`. A test that fails with `-c` but passes with
   `-c=1` conflicts with another test over a shared resource. A test that fails with `-c=1`
   too is an ordinary failure (or depends on another test that no longer runs first).
3. Find the partner by narrowing: run the failing test alone, then with a name filter
   (`mitts test -c db_`) that selects a group, then half of it.
4. Keep the plain `mitts test` for suites you cannot make independent, or run `-c=1`.
   Tests that must not run together can also live in one binary that is run on its own.
5. Use `-c` with `--no-fail-fast` in CI for the speed, and `--ignored` for the slow tests.

Writing tests that do not conflict:

- **Own your files.** Create a fresh directory per test (`tempfile::tempdir()`), put every file
  in it, and never write into the source tree, `target/` or a fixed `/tmp` name.
- **Let the OS pick the port.** Bind port `0` and read the port back; pass the address to the
  code under test instead of hard-coding it.
- **Isolate the environment.** Point the code at a per-test cache directory, `HOME` or config
  path (a temp directory) rather than the real one. Set environment variables on the child
  process you start, not on the test process, so nothing leaks.
- **Make names unique.** A database, table, queue or container name made of the test name plus
  a process id or random suffix. Create it in the test and drop it at the end (also when the
  test fails: use a guard object with `Drop`).
- **Make each test independent.** It builds its own input, assumes nothing about another test or
  about the order, and leaves nothing behind.
- **Do not sleep for a fixed time to wait for something.** Wait for the condition with a
  generous timeout and a short poll, and avoid asserting how long something took.
- **Do not rely on `static` locks for external resources.** If tests really must be
  exclusive, serialize them with something visible across processes (a file lock in a temp
  directory) or put them in a separate suite.
- **Keep one test cheap.** Many small tests parallelize well; a handful of very long ones set
  the total time whatever the pool size.

## Limits

A binary without the standard harness (`harness = false`) is run once as a whole; arguments after
`--` are not passed to the test processes; documentation tests run after the tests, as usual;
there are no retries, per-test timeouts or JUnit output yet.
