# RSScript self-hosting

Self-hosting research (the RSS-written lexer, parser, and checker, their parity
harness, and the C backend exploration) is archived on the
`archive/experiments-2026-09` branch together with the Rust AOT backend and the
REIR review integration. It is not a product goal and is not part of the root
workspace. A standalone compiler, stage1/stage2 bootstrap, and further C backend
expansion are not planned.

Shipping a prebuilt Rust compiler removes Rust from user installation. It does
not make compiler development self-hosting.
