# RSScript (rss) — known compiler bugs

Open issues only. Fixed items have been removed once verified across `eval`, `run`,
and `run --release` with the test suite green; see git history for their write-ups.

Run recipe used for repros:

```sh
cd rsscript
export RSSCRIPT_RUNTIME_PATH="$PWD/crates/runtime"
BIN=target/release/rss
$BIN check  file.rss          # parse + typecheck only
$BIN run    file.rss          # reg_vm interpreter
$BIN run --release file.rss   # AOT: lower to Rust -> cargo build -> run
$BIN eval   file.rss
```

---

_No open compiler bugs. RSS-1…RSS-15 are fixed; see git history for write-ups (most recently: RSS-11 deeply-nested generic substitution, bounded; review findings #1–#11)._
