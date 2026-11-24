# XCLayer Patch 19 — ownership, mapping and accounting follow-up

Baseline: the two supplied Patch 18 source archives. Scope: generic compatibility-layer memory, resource and I/O correctness; no game data, keys, licensing or protection changes. AI-use: GPT-6.2 (OpenAI).

## Validation status

The source and independent patch groups were inspected statically. The following commands were attempted from the repository root:

```
cargo build --workspace --locked
cargo test --workspace --locked -- --test-threads=4
cargo build --release --workspace --locked
```

All three returned exit code 127 (`cargo: command not found`). Tests executed: 0; pass/fail/skip counts unavailable, not a successful test run. No Windows execution, real GPU tests, retail-title tests, BCPack measurement, FPS or memory measurements. No GitHub CI run initiated; gh unavailable. No review of overlapping upstream PRs. `git diff --check` passed; six independently applied logical patches recreated the edited tree. Workflow YAML parsed successfully. No dependency changes, generated table edits or formatting pass.

## 19.01 — Memory: track observable title allocations separately

Root cause: process private bytes include host and driver allocations, while alias mappings and recommits can double-count memory. A single inferred total is not reliable.
Files: `hle/kernelx.rs`, new `hle/kernelx/accounting.rs`, `host.rs` (paths relative to crates/xcl/src).
Changes: maintain live default XMem allocations with reuse generation tokens and successful-free accounting; count logical live physical pages; track explicit commits within known plain Windows reservations as merged page intervals, including partial decommit and release. Preserve Win32 last error. Report incomplete/untracked operations. Diagnostics show separate components, not a fabricated aggregate.
Tests added: `default_xmem_ledger_preserves_reused_address_and_failed_free`, `recommits_partial_decommit_and_address_reuse_are_not_double_counted`.
Diff: +237/-15, three files. Commands/status: common commands above, none executed. Not tested: native allocation concurrency, ledger overhead, OOM handling on all bookkeeping paths, graphics/sparse accounting and alias attribution. TitleMemoryStatus and strict override remain unchanged. Full title-only accounting needs physical backing identity, all commit/decommit paths including VirtualAllocEx and sparse/UMA, consistent shared allocations, and measured validation. AI-use: GPT-6.2.

## 19.02 — Physical memory: stage fresh mappings and retain data on failed unmap

Root cause: partial destination commits and failure restoring a page's home can leave changed ownership or lose contents.
Files: `hle/kernelx.rs`, `hle/kernelx/placeholders.rs`.
Changes: validate complete destination ranges, reject invalid or protected destinations; stage commits for fresh plain mappings up to 64 MiB before moving ownership, undo successful staging commits on a later failure and log rollback failure. Leave existing mapping intact if recommitting its home fails; propagate error instead of freeing the reservation. No arbitrary large transaction snapshots.
Tests added: `failed_home_commit_keeps_the_live_mapping`, `staging_rolls_back_only_successful_commits_on_failure`, `mapping_destination_checks_the_entire_range`.
Diff: +220/-42, two files. Commands/status: common commands above, none executed. Not tested: real injected Windows allocation failure across the full transaction (helper test uses fake commit operation), concurrent remap/free, full lock-order proof, sparse or placeholder rollback, overlapping or >64 MiB mappings, recovery after rollback itself fails. Earlier pages in a multi-page unmap may already have moved before a later failure. Full transactional rollback remains open. AI-use: GPT-6.2.

## 19.03 — D3D12.X: own queue snapshots and seal only captured release cohorts

Root cause: a borrowed queue address could outlive the registry lock; releases appended after a fence snapshot could be attached to an earlier batch.
Files: `hle/d3d12x/present.rs`, `late_release.rs`, `real.rs`.
Changes: clone owning COM queue references while holding the registry lock; register queue before submission/presentation; capture generation and pending count before taking the snapshot; only seal the captured prefix if generation still matches. Reject a stale competing seal.
Test added: `queue_snapshot_only_seals_its_original_cohort`.
Diff: +50/-24, three files. Commands/status: common commands above, none executed. Not tested: device recreation, all submission paths, real fence/queue races. Permanent registry reference and retirement of devices/queues remain open; do not infer title ownership from a COM Release result when internal refs exist. Need explicit ownership registry with tracked title/internal refs, retirement across presenter, texture upload and fence users, and concurrent device-recreation tests. AI-use: GPT-6.2.

## 19.04 — D3D12.X: isolate resource cache by device and validate address-only ranges

Root cause: same address/description across devices could collide; racing creations could overwrite cached ownership. Unchecked buffer ranges may wrap or become unrepresentable.
File: `hle/d3d12x/real.rs`.
Changes: include device interface address in MADE key, serialize misses with a creation gate and recheck, preserve first successful entry, check descriptor range and placed-resource arithmetic, do not record a failed sparse creation.
Tests added: `made_cache_keeps_first_resource_and_separates_devices`, `address_only_buffer_descriptors_reject_unrepresentable_ranges`; existing test key adapted.
Diff: +48/-11, one file. Commands/status: common commands above, none executed. Not tested: creation gate lock-order/reentrancy and contention, real remap and device destruction, non-UMA view coherency. MADE remains unbounded and cannot yet be safely invalidated or evicted; descriptor slots do not retain COM references automatically. Need descriptor ownership across copies, generation of backing allocations/remaps, explicit slot replacement, late retirement across all relevant queues; only then bound inactive entries. Device interface address is not a complete device generation identifier. Address-only bounds do not establish actual allocation extent. Full non-UMA support remains open, particularly SRV/UAV coherence and GPU writeback. AI-use: GPT-6.2.

## 19.05 — Native file I/O: prepare lazy buffers for title imports

Root cause: direct title calls to NtReadFile/NtWriteFile can pass uncommitted lazy buffers to the kernel.
Files: `vfs_hooks.rs`, `resolver.rs`.
Changes: route direct named title imports and title GetProcAddress resolution through wrappers around the actual ntdll APIs. Prepare buffer, IO_STATUS_BLOCK, optional offset and key; preserve native result and last error. Existing explicit resolver aliases take precedence. No process-wide ntdll hooks.
Test added: `native_file_io_commits_buffers_and_forwards_status` (fake native callback, not real kernel I/O).
Diff: +78/-2, two files. Commands/status: common commands above, none executed. Not tested: native Windows asynchronous lifetime and pending completion, APCs, raw syscall paths, indirect imports, NtCreateFile path translation, scatter/gather and other Nt APIs. Native I/O coverage is partial. AI-use: GPT-6.2.

## 19.06 — CI: update validation and release artifact names

Root cause: need a repeatable compiler/test gate and release artifacts for this revision.
Files: `.github/workflows/patch18-build.yml`, `README.md`.
Changes: update existing workflow in place to display Patch 19 and produce XCL-Patch19-logs / XCL-Patch19-Windows; preserve filename to avoid creating duplicate workflow on browser upload. Existing Windows CI constraints and exclusions remain.
Tests: YAML parse (passed); no new Rust test for documentation/workflow naming. Diff: +18/-10, two files. Commands/status: common commands above, none executed. Not tested: actual CI run and artifact contents. AI-use: GPT-6.2.

## Handoff priorities

1. Run Windows MSVC CI first; fix compile failures and verify all nine added regression tests without weakening assertions. Review native ABI, unsafe boundaries and lock order before running real GPU tests.
2. Complete accounting with allocation identities and backing vs alias counts. Do not switch TitleMemoryStatus to a partial sum.
3. Design descriptor ownership and resource generation tracking before eviction; queue retirement and device lifetime must agree with the same fence model.
4. Extend physical-map transactions to sparse, overlapping and larger ranges with failure-injection tests and explicit rollback guarantees.
5. Validate native file wrappers on actual Windows, including async completion, then cover only proven missing APIs.
6. Design non-UMA coherent backing and synchronization on actual hardware; measure rather than invent performance results. BCPack and unknown mesh ABI remain unresolved.

The source diff contains 651 insertions, 104 deletions in 11 files, excluding this report. Six patch groups, largest 262 changed lines. Nine Rust tests added, zero executed. This is an uncompiled candidate, not a verified replacement for the last green CI build.
