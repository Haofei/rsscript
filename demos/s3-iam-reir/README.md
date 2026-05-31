# S3 IAM REIR demo

This demo shows the core REIR loop:

1. RSScript code uploads reports to S3 through an async package facade.
2. `rsspkg.toml` binds the facade symbol `S3.put_object` to one external capability.
3. Package review propagates that capability through the RSScript call graph.
4. REIR reconciles required code capabilities against mock IAM grants before deploy.

The RSS source does not write `effects(requires(...))`. The capability binding is declared once at the package boundary.
`interface/s3.rssi` is the mock native S3 boundary. `src/upload.rss` is ordinary RSS async business code; it is not a native implementation.
The runtime path is also executable: `rsscript-runtime` owns the Tokio-backed native async executor, while the demo native wrapper only starts HTTP IO futures.

## Run the full demo flow with the Rust test runner

```sh
cargo test --test s3_iam_reir_demo_e2e -- --nocapture
```

Expected flow:

1. REIR fails before deploy because mock IAM grants `s3:GetObject`, not `s3:PutObject`.
2. The fixed IAM mock grants `s3:PutObject`, and REIR passes.
3. The RSS async uploader sends six 256 KiB objects through `rsscript-runtime::spawn_tokio_native`.
4. A blocking sync client uploads the same number and size of objects sequentially.

The test runner starts the Tokio mock S3 server, builds the generated RSS package, times a warmed async upload, times the blocking sync client, and asserts that the server saw overlapping async requests. It does not use shell scripts.

## Run the failing deployment check

From the repository root:

```sh
mkdir -p demos/s3-iam-reir/review

cargo run --bin rss -- pkg review --reir demos/s3-iam-reir \
  > demos/s3-iam-reir/review/rsscript.json

demos/s3-iam-reir/scripts/mock-iam-to-reir.sh missing \
  > demos/s3-iam-reir/review/iam-missing.json

cargo run -p reir -- merge \
  demos/s3-iam-reir/review/rsscript.json \
  demos/s3-iam-reir/review/iam-missing.json \
  --out demos/s3-iam-reir/review/system-missing.json

cargo run -p reir -- reconcile \
  --target prod \
  --out demos/s3-iam-reir/review/system-missing-reconciled.json \
  demos/s3-iam-reir/review/system-missing.json
```

Expected result: reconciliation fails because mock IAM grants `s3:GetObject`, while the RSS package requires `s3:PutObject`.
The mock runtime grants still cover `runtime.native` and `network.client`, so the failure is isolated to the missing S3 IAM action.

The missing capability evidence points back to the RSS call site, for example `src/upload.rss` inside `upload_report`, with the propagated call chain `upload_report -> S3.put_object`.

## Run the fixed deployment check

```sh
demos/s3-iam-reir/scripts/mock-iam-to-reir.sh fixed \
  > demos/s3-iam-reir/review/iam-fixed.json

cargo run -p reir -- merge \
  demos/s3-iam-reir/review/rsscript.json \
  demos/s3-iam-reir/review/iam-fixed.json \
  --out demos/s3-iam-reir/review/system-fixed.json

cargo run -p reir -- reconcile \
  --target prod \
  --out demos/s3-iam-reir/review/system-fixed-reconciled.json \
  demos/s3-iam-reir/review/system-fixed.json
```

Expected result: the `s3:PutObject` requirement is covered.

## Async note

`Reports.upload_batch` uses `task_group` with multiple `async let` uploads. This is structured RSScript concurrency. RSScript source does not expose Tokio, Future, Pin, Poll, or Waker; those remain runtime implementation details.
