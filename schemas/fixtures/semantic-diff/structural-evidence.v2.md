## RSScript semantic diff

- Old module: `sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb`
- New module: `sha256:1111111111111111111111111111111111111111111111111111111111111111`

### Imports

- Added: 0
- Removed: 0
- Changed: 0

### External contracts

- Added: 0
- Removed: 0
- Changed: 0

### Exports

- Added: 0
- Removed: 0
- Changed: 0

### External calls

- Added: 0
- Removed: 0
- Changed: 0

### Call graph

- Added: 1
- Removed: 0
- Changed: 0
  - Added `{"caller":"main","callee":"publish"}`

### Recursive functions

- Added: 0
- Removed: 0
- Changed: 0

### Resource lifetimes

- Added: 1
- Removed: 0
- Changed: 0
  - Added `{"function":"main","binding":"file","acquisition":"with","cleanup":"scope_exit","cleanup_on_cancellation":true}`

### Resource transfers

- Added: 1
- Removed: 0
- Changed: 0
  - Added `{"function":"main","binding":"file","operation":"take"}`

### Task groups

- Added: 1
- Removed: 0
- Changed: 0
  - Added `{"function":"main","spawned_tasks":2,"select_arms":1,"drains_on_exit":true,"cleanup_on_cancellation":true}`

### Await sites

- Added: 0
- Removed: 0
- Changed: 0

### Diagnostics

- Added: 0
- Removed: 0
- Changed: 0

### Summary counters

- `source_files`: 1 → 2
