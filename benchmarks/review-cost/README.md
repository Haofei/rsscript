# Review-Cost Benchmark

Measures how RSScript's review map reduces the human review surface.

## Run

```sh
./benchmarks/review-cost/run.sh          # human-readable table
./benchmarks/review-cost/run.sh --json   # machine-readable for CI
```

## What it measures

For each scenario:

| Metric | Meaning |
|--------|---------|
| Lines | Total semantic lines (excl. blank/comments) |
| Must | Lines requiring human review (mutations, retentions, resources, native calls) |
| Fold | Lines a reviewer can safely skip (pure reads, struct defs, type-only code) |
| Unk | Lines with unknown/unresolved risk |
| Saved | Percentage of lines folded away |

## Interpretation

A reviewer looking at a raw diff sees **all** lines. With RSScript review maps:

- **Must-review lines** are semantically significant (state changes, resource ops, retentions)
- **Foldable lines** are proven low-risk by the compiler (pure reads, no side effects)
- **Unknown lines** must NEVER be hidden — they represent unresolved risk

The `review_ratio` is `must_review / total`. Lower = less review burden.

## CI integration

Use `--json` output with the stable REIR CI schema:

```yaml
- name: Review cost check
  run: |
    result=$(./benchmarks/review-cost/run.sh --json)
    echo "$result" | jq '.scenarios[] | select(.unknown_lines > 0)'
```

## Adding scenarios

Add new `.rss` files to the `SCENARIOS` array in `run.sh`.
Good scenarios have a mix of:
- Pure computation (should be foldable)
- Mutations and resource access (should be must-review)
- External/native boundaries (may be unknown)
