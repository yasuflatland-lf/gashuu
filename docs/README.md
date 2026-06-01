# gashuu documentation

The project documentation is organized in three layers: the root `CLAUDE.md` is the L1 entry point with the project overview and non-negotiable rules; these topic docs (L2) cover policy, decisions, and constraints by area; and `###` how-to sections within each doc provide detailed guidance. Architecture Decision Records (ADRs) in `ADRs/` hold the canonical "why" for all design decisions.

| Doc | What it covers | When to read |
|-----|----------------|--------------|
| [architecture.md](architecture.md) | As-built module map of the two crates; decisions link out to ADRs | Before working on core or UI; to understand the layer boundary |
| [toolchain.md](toolchain.md) | Rust/mise pin, system libraries, dependency constraints (zip, unrar), build notes | When setting up the environment or adding dependencies |
| [quality-gates.md](quality-gates.md) | The fmt/clippy/nextest gates and coverage; run before calling work done | Before pushing a change; to verify all gates pass |
| [conventions.md](conventions.md) | Language, TDD, PR size, test-fixture rules | When starting a new feature or PR |
| [patterns.md](patterns.md) | Hard-won patterns & gotchas from real issues | Before editing a specific area (cross-crate mocking, zoom/pan, settings, etc.) |
| [scope.md](scope.md) | Shipped baseline and intentionally deferred work | To check if a feature is in scope or deferred |
| [ADRs/README.md](ADRs/README.md) | Architecture Decision Records; canonical "why" for all design decisions | When understanding the rationale behind a design choice |
| [superpowers/](superpowers/) | Specs workspace | For collaborative planning and brainstorming sessions |
