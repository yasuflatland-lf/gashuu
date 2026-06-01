# gashuu — Project Guide for Claude

gashuu is a cross-platform manga viewer in Rust + Slint, built as a two-crate Cargo
workspace: `gashuu-core` (headless domain + I/O) and `gashuu` (Slint presentation).
This file is the L1 entry point; detailed conventions and hard-won gotchas live under
`docs/`. **User instructions always override this file.**

**Read the relevant doc below BEFORE working in that area — the detail lives there, not here.**

## Documentation map

| Topic | Doc | Read before |
| --- | --- | --- |
| Module map (as-built) | [docs/architecture.md](docs/architecture.md) | adding or moving modules |
| Decisions (the "why") | [docs/ADRs/](docs/ADRs/README.md) | questioning a design choice |
| Toolchain, build, deps | [docs/toolchain.md](docs/toolchain.md) | build / CI / dependency work |
| Quality gates & coverage | [docs/quality-gates.md](docs/quality-gates.md) | calling any change "done" |
| Code conventions | [docs/conventions.md](docs/conventions.md) | writing any code |
| Patterns & gotchas | [docs/patterns.md](docs/patterns.md) | **editing core or UI logic** |
| Scope: shipped / deferred | [docs/scope.md](docs/scope.md) | scoping a feature |

## Non-negotiable rules

- Run every cargo command through the pin: `mise exec -- cargo <...>`.
- A change is not done until all three gates are green (see docs/quality-gates.md):
  `mise exec -- cargo fmt --check` · `mise exec -- cargo clippy --workspace --all-targets -- -D warnings` · `mise exec -- cargo nextest run --workspace --profile ci`.
- All comments, identifiers, and docs in **English**.
- `gashuu-core` stays headless — no `slint`, no `tracing`; the core↔UI boundary is RGBA bytes + dimensions.
- TDD; keep the crate compiling at every save; keep a PR ≤ ~1000 production LOC.
