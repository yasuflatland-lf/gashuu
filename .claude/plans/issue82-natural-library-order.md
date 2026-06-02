# Issue 82 Natural Library Order Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development for execution. Main agent is orchestration-only: do not directly implement production/test code from the controller session. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Sort the library and carousel by natural, numeric-aware title order, with canonical path as a stable tie-breaker, and focus newly added books at their sorted position.

**Architecture:** Core `Library` becomes the single source of truth for book ordering. `natural_cmp` moves from private `page_source::naming` into shared core `ordering`, and UI model/cover rows keep inheriting `Library::books()` order. The add flow returns the canonical paths actually inserted so the UI can focus the first new book after the sorted rebuild.

**Tech Stack:** Rust workspace (`gashuu-core`, `gashuu`), Slint presentation, serde JSON persistence, mise-pinned Cargo commands.

---

## Progress Log

- [x] 2026-06-03: Issue #82 inspected through GitHub; scope confirmed.
- [x] 2026-06-03: Explorer agents classified core, UI, and tooling surfaces.
- [x] Task A1: Extract shared natural ordering module.
- [x] Task A2: Make `Library` naturally ordered and normalize load.
- [x] Task B1: Update presentation ordering tests that inherit core order.
- [x] Task B2: Focus newly added book at sorted index after add.
- [x] Task C1: Run focused verification and full gates.
- [ ] Task D1: Commit grouped changes with dedicated commit agent. Core commit complete: `840a208 feat(core): sort library by natural title order`; UI commit complete: `8a22673 fix(ui): focus added book after sorted insert`.
- [ ] Task E1: Final review and issue acceptance check.

## Agent And Commit Rules

- Main agent role: orchestration, plan/progress updates, subagent dispatch, review coordination. Do not directly edit production or test code.
- Commit agent role: one dedicated worker handles all `git add` and `git commit` operations. Implementation workers must not commit.
- Implementation workers must not revert unrelated edits and must assume other agents may be working in the repo.
- Every shell command must be prefixed with `rtk`.
- Every Cargo command must use the mise pin: `rtk mise exec -- cargo ...`.
- TDD is required for behavior changes: write/update tests first, observe the expected failure, then implement.
- If a subagent context drops below 5%, the subagent must compact, preserving discovered solutions and removing debugging transcript.

## Task Ordering Rationale

Tasks are ordered by **high impact x low change volume**, then by parallel safety:

1. **A1 shared `ordering` extraction**: small mechanical move; unblocks core Library ordering.
2. **A2 Library invariant + load normalization**: highest behavior impact; depends on A1.
3. **B1 presentation inherited-order test updates**: independent from B2 once A2 exists; validates row/model inheritance.
4. **B2 add focus correction**: UI-specific and independent from B1 after A2; fixes required sorted-insertion UX.
5. **C1 verification**: depends on all code changes.
6. **D1 commits**: serialized through one commit worker after each green task group.
7. **E1 final review**: depends on verification and commits.

## Independent Clusters

### Cluster A: Core Ordering And Persistence

**Dependency:** none for A1; A2 depends on A1.

**Files:**
- Create: `crates/gashuu-core/src/ordering.rs`
- Modify: `crates/gashuu-core/src/lib.rs`
- Modify: `crates/gashuu-core/src/page_source/naming.rs`
- Modify: `crates/gashuu-core/src/page_source/folder.rs`
- Modify: `crates/gashuu-core/src/page_source/zip.rs`
- Modify: `crates/gashuu-core/src/page_source/rar.rs`
- Modify: `crates/gashuu-core/src/library.rs`
- Modify: `crates/gashuu-core/src/library_store.rs`

**Model guidance:** A1 can use `gpt-5.4-mini medium`; A2 should use `gpt-5.5 xhigh` because it changes the aggregate invariant and deserialization path.

### Cluster B: Presentation Inheritance And Focus

**Dependency:** depends on Cluster A because tests should assert the new `Library::books()` order.

**Files:**
- Modify: `crates/gashuu/src/library_model.rs`
- Modify: `crates/gashuu/src/carousel.rs`
- Modify: `crates/gashuu/src/main.rs`

**Model guidance:** B1 can use `gpt-5.4-mini low`; B2 should use `gpt-5.4-mini medium`.

### Cluster C: Verification

**Dependency:** all implementation clusters complete.

**Files:** no intended writes except generated test output none.

**Model guidance:** `gpt-5.4-mini low` for command execution; escalate if failures require reasoning.

### Cluster D: Commit Serialization

**Dependency:** invoked after each reviewed task group.

**Files:** Git index only.

**Model guidance:** one `gpt-5.4-mini low` commit worker reused for all commits.

### Cluster E: Final Review

**Dependency:** all code and commits complete.

**Files:** read-only.

**Model guidance:** `gpt-5.5 xhigh` final reviewer.

---

## Task A1: Extract Shared Natural Ordering Module

**Impact:** high reuse unblock, small mechanical change.

**Write ownership:**
- `crates/gashuu-core/src/ordering.rs`
- `crates/gashuu-core/src/lib.rs`
- `crates/gashuu-core/src/page_source/naming.rs`
- `crates/gashuu-core/src/page_source/folder.rs`
- `crates/gashuu-core/src/page_source/zip.rs`
- `crates/gashuu-core/src/page_source/rar.rs`

- [x] **Step 1: Move tests first**

Create `crates/gashuu-core/src/ordering.rs` with the existing `natural_cmp`, `take_digits`, `cmp_numeric`, and the existing `natural_cmp_tests` moved from `page_source/naming.rs`. Keep function visibility `pub(crate)` for `natural_cmp` and private for helpers.

- [x] **Step 2: Verify compile failure before imports are updated**

Run:

```bash
rtk mise exec -- cargo test -p gashuu-core natural_cmp --lib
```

Expected: fail until `ordering` is exposed or import paths are fixed.

- [x] **Step 3: Expose and update imports**

Add `pub(crate) mod ordering;` in `crates/gashuu-core/src/lib.rs`. Update page-source users from `super::naming::natural_cmp` to `crate::ordering::natural_cmp`. Keep `has_image_ext`, `IMAGE_EXTS`, `MAX_ENTRY_BYTES`, and archive path helpers in `naming.rs`.

- [x] **Step 4: Run focused tests**

Run:

```bash
rtk mise exec -- cargo test -p gashuu-core natural_cmp --lib
```

Expected: natural comparison tests pass.

- [x] **Step 5: Report without committing**

Implementation worker reports changed files, test output, and any import cleanup concerns. Do not commit.

## Task A2: Make Library Naturally Ordered And Normalize Load

**Impact:** highest behavior change; limited core files.

**Depends on:** Task A1.

**Write ownership:**
- `crates/gashuu-core/src/library.rs`
- `crates/gashuu-core/src/library_store.rs`

- [x] **Step 1: Write/update failing library ordering tests**

Update the old insertion-order test in `library.rs` so adding `vol 10`, `vol 1`, `vol 2` asserts `vol 1`, `vol 2`, `vol 10`. Add a same-title deterministic tie-break test that creates two books with the same title and different paths and asserts ascending canonical path order.

- [x] **Step 2: Write failing load-normalization test**

In `library_store.rs`, add a test that deserializes an old unsorted JSON library containing `vol 10`, `vol 1`, `vol 2` and asserts the loaded `Library::books()` order is natural. The JSON should match the existing persisted shape and use temporary paths or stable string paths already accepted by current tests.

- [x] **Step 3: Run focused tests and observe failures**

Run:

```bash
rtk mise exec -- cargo test -p gashuu-core library --lib
rtk mise exec -- cargo test -p gashuu-core library_store --lib
```

Expected: new/updated tests fail because `Library` still preserves insertion/deserialized order.

- [x] **Step 4: Implement core ordering invariant**

In `library.rs`, add:

```rust
fn book_order(a: &Book, b: &Book) -> std::cmp::Ordering {
    crate::ordering::natural_cmp(a.title(), b.title())
        .then_with(|| a.path().as_os_str().cmp(b.path().as_os_str()))
}
```

After successful `push` in `Library::add`, call `self.books.sort_by(book_order)`. Add `pub(crate) fn normalize(&mut self)` that sorts the vector with `book_order`. Update doc comments from insertion order to natural title order with canonical-path tie-break.

- [x] **Step 5: Normalize deserialized libraries**

In `library_store.rs`, after `serde_json::from_str::<Library>(...)`, call `library.normalize()` before returning.

- [x] **Step 6: Run focused tests**

Run:

```bash
rtk mise exec -- cargo test -p gashuu-core library --lib
rtk mise exec -- cargo test -p gashuu-core library_store --lib
```

Expected: updated ordering and load-normalization tests pass.

- [x] **Step 7: Report without committing**

Implementation worker reports changed files, test output, and any concerns. Do not commit.

## Task B1: Update Presentation Model Ordering Expectations

**Impact:** validates UI inherits core order; small test-only/presentation scope.

**Depends on:** Task A2.

**Write ownership:**
- `crates/gashuu/src/library_model.rs`
- `crates/gashuu/src/carousel.rs`

- [x] **Step 1: Update failing presentation tests**

Rename/update `carousel_data_preserves_insertion_order` to assert natural order inherited from `Library`. Review cover request order-dependent tests and update expected rows only if the sorted `Library::books()` order changes the setup.

- [x] **Step 2: Run focused tests and observe failures if implementation is not integrated**

Run:

```bash
rtk mise exec -- cargo test -p gashuu library_model --lib
rtk mise exec -- cargo test -p gashuu carousel --lib
```

Expected after A2: tests pass with updated expectations. If they fail, the failure should identify stale insertion-order assumptions.

- [x] **Step 3: Report without committing**

Implementation worker reports changed files and focused test output. Do not commit.

## Task B2: Focus Newly Added Book At Sorted Index

**Impact:** required UX fix after sorting; localized UI flow.

**Depends on:** Task A2.

**Write ownership:**
- `crates/gashuu/src/main.rs`

- [x] **Step 1: Update failing add-path tests**

Change `add_paths_preserves_insertion_order` to assert `add_paths` returns the canonical paths that were actually inserted. Include a duplicate path case to prove duplicates are not returned. If a pure helper is needed for finding the focus index, add a focused test for it in `main.rs`.

- [x] **Step 2: Run focused main tests and observe failure**

Run:

```bash
rtk mise exec -- cargo test -p gashuu add_paths --lib
```

Expected: tests fail because `add_paths` currently returns only a count.

- [x] **Step 3: Change `add_paths` return type**

Change `fn add_paths(lib: &mut Library, paths: Vec<PathBuf>) -> usize` to return `Vec<PathBuf>` of canonical paths actually added. Preserve current canonicalization behavior through `Library::add` by returning `book.path().to_path_buf()` only when `lib.add(path)` returns `true`.

- [x] **Step 4: Focus the sorted position after rebuild**

In `add_books_and_refresh`, store `added_paths`. After rebuilding the carousel model, find `added_paths.first()` in `library.borrow().books()` by canonical path. If found, set `ui.set_carousel_focused_index(index as i32)` before `ui.invoke_focus_carousel()`. Keep status wording based on `added_paths.len()`.

- [x] **Step 5: Run focused tests**

Run:

```bash
rtk mise exec -- cargo test -p gashuu add_paths --lib
```

Expected: add-path tests pass.

- [x] **Step 6: Report without committing**

Implementation worker reports changed files, focused test output, and whether any UI focus path could not be headlessly tested. Do not commit.

## Task C1: Verification Gates

**Impact:** validates entire issue acceptance.

**Depends on:** Tasks A1, A2, B1, B2 reviewed.

**Write ownership:** none.

- [x] **Step 1: Run format**

Run:

```bash
rtk mise exec -- cargo fmt --check
```

Expected: exit 0.

- [x] **Step 2: Run clippy**

Run:

```bash
rtk mise exec -- cargo clippy --workspace --all-targets -- -D warnings
```

Expected: exit 0.

- [x] **Step 3: Run nextest**

Run:

```bash
rtk mise exec -- cargo nextest run --workspace --profile ci
```

Expected: exit 0.

- [x] **Step 4: Report exact gate status**

Verification worker reports the exact commands and pass/fail status. If any gate fails, report blocker details and do not mark complete.

## Task D1: Dedicated Commit Agent

**Impact:** prevents commit/index conflicts.

**Depends on:** Invoked after reviewed green task groups.

**Write ownership:** Git index and commits only.

- [ ] **Step 1: Inspect status**

Run:

```bash
rtk git status --short
rtk git diff --stat
```

- [x] **Step 2: Commit A group when reviewed**

After A1/A2 pass focused tests and review, commit only core files with message:

```text
feat(core): sort library by natural title order
```

- [x] **Step 3: Commit B group when reviewed**

After B1/B2 pass focused tests and review, commit only UI files with message:

```text
fix(ui): focus added book after sorted insert
```

- [ ] **Step 4: Commit verification/plan updates when complete**

After full gates and final plan update, commit the plan/progress file if the user wants it committed with message:

```text
docs: track issue 82 implementation plan
```

## Task E1: Final Review And Acceptance Check

**Impact:** catches missed requirement before final response.

**Depends on:** Tasks A-D.

**Write ownership:** read-only unless reviewer finds required fixes.

- [ ] **Step 1: Review acceptance criteria**

Check:
- Natural title order is numeric-aware: `vol 1 < vol 2 < vol 10`.
- Same-title ordering is stable by canonical path.
- Deserialized old unsorted libraries normalize on load.
- Presentation model and cover requests inherit `Library::books()` order.
- Adding a book focuses the added book at its sorted position.
- No user-configurable ordering was added.

- [ ] **Step 2: Review git diff and commits**

Run:

```bash
rtk git status --short --branch
rtk git log --oneline -5
```

- [ ] **Step 3: Report final outcome**

Report commits, gates, and any residual risk.
