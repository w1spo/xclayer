# XCLayer — Patch 18 engineering handoff

**Baseline:** supplied `XCLayer_Patch17_Part_01.zip` and `XCLayer_Patch17_Part_02.zip`, plus `PATCH17_AUDIT.md`. This is a static-review candidate, not a validated Windows release. Patch 17 itself has not been compiled in this environment. No retail title was executed. The previous Patch 16 CI result is not evidence that Patch 18 passes.

## Patch split and verification

The following are independent sequential changes (each under 400 changed lines), in the proposed split. Paths below are relative to `crates/xcl/src/` unless specified. There are 10 added test functions; some are helper-level regression tests for fixes already present in Patch 17, not newly discovered defects. No dependency, generated-table, or test assertion weakening changes. No game files, keys or protection changes. One existing test fixture was corrected to provide two input elements when passing a count of two.

**Commands actually run, shared by every entry below:**

- `cargo build --workspace --locked` — shell exit 127, `cargo: command not found`; compiler never started.
- `cargo test --workspace -- --test-threads=4` — shell exit 127, `cargo: command not found`; runner never started. Actual tests executed: 0; pass/fail/skip counts unavailable (not a passing test run).
- `git diff --cached --check` — passed.
- On a newly extracted copy of Patch 17, `git apply --check <patch>` and `git apply <patch>` for each of the 11 sequential diffs — all passed. `diff -qr --exclude=.git /tmp/xcl18 /tmp/xcl18verify` — identical source trees before adding this report.
- Parsed the new workflow using Python `yaml.BaseLoader`, checked the dispatch key and Windows runner — passed. This is syntax inspection, not a GitHub runner execution.

For each entry, these shared command outcomes apply. No individual Rust test was run locally. AI use: GPT-6.2.

### 01. Preflight: warn about low system commit headroom (+33/-1)
Root cause: free physical RAM is not the system commit budget; a title may exhaust commit before it exhausts RAM. Added a separate `K32GetPerformanceInfo` probe and saturating arithmetic. Warning threshold is 24 GiB, not a hard minimum or an assertion that a title needs exactly that amount. No startup refusal and no change to `Found`.

Files: `preflight.rs`, root `README.md`. Test: `low_system_commit_is_a_warning_not_a_startup_error` (threshold and saturating underflow). Not tested: real Windows API call, constrained page file. AI use: GPT-6.2.

### 02. KernelX: make existing title-memory policy testable (+16/-1)
Root cause: the existing Patch 17 workaround lacks coverage. Extracted the calculation and tested default and strict behavior. **This does not implement title-attributable accounting.** `XCL_TITLE_MEMORY_STRICT=1` still uses process-private bytes capped at total title budget; default reports at least one quarter available. It is an approximation, not a measurement of title memory.

File: `hle/kernelx.rs`. Test: `title_memory_policy_default_and_strict`. Not tested: full env-var integration, true allocation attribution. AI use: GPT-6.2.

### 03. KernelX: test failed lazy commits without exhausting system memory (+17/-1)
Root cause: Patch 17 already restores the lazy marker on allocation failure, but had no deterministic test. Extracted the operation with a callback so failure and retry can be simulated under the same set lock.

File: `hle/kernelx/lazy.rs`. Test: `failed_commit_keeps_the_lazy_page`. Not tested: actual Windows commit failure and exception-handler reentry. AI use: GPT-6.2.

### 04. KernelX: reject mappings of individually freed physical pages (+44/-9)
Root cause: duplicate-free protection did not prevent `page_home` from resolving a freed page in a block with surviving neighbours. A concurrent map could also occur between dropping the mapping and marking ownership freed. Check `FREED_PAGES` and hold `AWAY` across the ownership update, dropping block/set locks before releasing homes.

File: `hle/kernelx.rs`. Test: `freed_physical_page_cannot_be_mapped_or_free_its_live_neighbour`. Not tested: concurrent stress and GPU-backed home paths. AI use: GPT-6.2.

### 05. KernelX: serialize reservation release with physical mapping (+63/-20)
Root cause: releasing plain reservations, current-process `VirtualFreeEx` and replaced placeholders did not consistently serialize cleanup with `AWAY`; an address that is only partly readable must not be copied back. Hold the mapping guard through native release, extend cleanup to those release paths, check the full copy range and avoid a same-address copy. Handle lazy entries that have already been committed. Use checked progression for `VirtualQuery` ranges.

Files: `hle/kernelx.rs`, `hle/kernelx/placeholders.rs`. Tests: `releasing_a_reservation_returns_its_physical_pages`, `remapping_a_decommitted_physical_page_does_not_read_it`. Not tested: all placeholder operations, a real handle to the current process (rather than its pseudo-handle), failed native release, coherent-window release semantics, partial decommit, concurrent GPU traffic. In particular, cleanup ahead of a failing native release is not transactional; a failed home commit can still lose the only copy. AI use: GPT-6.2.

### 06. KernelX: honor large graphics allocation alignment (+29/-1)
Root cause: normal `VirtualAlloc` fallback guarantees allocation-granularity alignment, not 4 MiB alignment. Use `VirtualAlloc2` with `MEM_ADDRESS_REQUIREMENTS` for larger alignment. Preserve the true allocation base for normal release.

File: `hle/kernelx.rs`. Test: `windows_graphics_fallback_honours_four_mib_alignment`. Not tested: Windows linking and API availability, title-level XMem behavior. AI use: GPT-6.2.

### 07. KernelX: validate physical mapping sizes and consecutive homes (+39/-4)
Root cause: count/address arithmetic could overflow, and consecutive page numbers alone do not guarantee contiguous homes. Validate sizes before array access, use fallible vector reservation, verify the actual homes. Correct an existing test fixture whose array was shorter than its declared count.

File: `hle/kernelx.rs`. Test: `physical_mapping_rejects_wrapping_sizes_and_addresses`. Not tested: injected allocation failure during the mapping loop. **Full mid-loop rollback is not implemented.** AI use: GPT-6.2.

### 08. VFS: commit lazy file-I/O buffers before entering the kernel (+73/-6)
Root cause: the kernel does not necessarily invoke the process's user-mode lazy-page handler while touching an I/O buffer. Extend `ReadFile`/`ReadFileEx` preparation to `WriteFile`, `WriteFileEx`, and `ReadFileScatter`; use the host page size for scatter elements and validate the array before reading it. Preserve last-error around preparation.

File: `vfs_hooks.rs`. Test: `file_buffers_and_scatter_segments_commit_lazy_pages_before_io` (helper-level coverage). Not tested: actual hook installation, overlapped I/O, asynchronous buffer lifetime, file-system semantics and invalid input stress. `WriteFileGather` and direct `NtReadFile`, `NtWriteFile`, scatter/gather or other `Nt*` calls are not hooked. No direct Nt-call integration test was added. AI use: GPT-6.2.

### 09. D3D12.X: keep failed placed-resource creation out of MADE (+22/-5)
Root cause: Patch 17 already avoids caching null resources; verify both the HRESULT and returned pointer before insertion and add a deterministic test. A failed key is eligible for a later attempt.

File: `hle/d3d12x/real.rs`. Test: `failed_resource_creation_is_not_cached`. Not tested: actual COM failure/retry or concurrent cache misses. **Cache capacity, memory-generation invalidation, device identity and lifetime-safe eviction are not implemented.** AI use: GPT-6.2.

### 10. D3D12.X: batch deferred resource release instead of spinning (+23/-13)
Root cause: `late_release::hold` spins for up to 300 microseconds and repeatedly clones queue references per resource. Try synchronous draining only at the start of a backlog; otherwise use the existing periodic worker. Retain fence completion requirements.

File: `hle/d3d12x/late_release.rs`. Test: `only_the_first_release_in_a_backlog_requests_an_immediate_seal`. Not tested: real GPU fence timing, resource release followed immediately by heap decommit, queue creation races, worker scheduling and memory pressure. This may increase the delay before idle resources are unpinned; do not claim a measured performance improvement. AI use: GPT-6.2.

### 11. CI: validate Windows debug and release builds and retain logs (+65/-0)
Root cause: local standard-user compilation was blocked by device policy, and CI artifacts are needed for diagnosis. Add a manually dispatched workflow, leaving upstream CI unchanged. The release is gated on debug compilation and the test step; logs are retained even on failure.

File: `.github/workflows/patch18-build.yml`. Validation: YAML parse (not a Rust regression test). CI commands: `cargo build --workspace --locked`; `cargo test --workspace --locked -- --test-threads=4 --skip test_title_runs_and_game_folder_is_untouched`; `cargo build --release --workspace --locked`. The end-to-end exclusion matches the existing CI exclusion; no new test was disabled. Not tested: actual GitHub run, artifact upload, MSVC build. AI use: GPT-6.2.

## Patch 17 audit review

| Original change | Review outcome |
|---|---|
| 1. `unmap_away` avoids reading decommitted memory | Extended to full readable range and same-address avoidance. Concurrent native decommit outside the hook remains possible. |
| 2. `VirtualFree` cleans `AWAY` and `LAZY` | Added serialization and additional release paths. Native failure is still not transactional; partial decommit and coherent cases need further tests. |
| 3. `FREED_PAGES` duplicate-free guard | Added validity check to `page_home` and closed map/free lock gap. Reused page numbers are not generation-tagged. |
| 4. `TitleMemoryStatus` 75% cap | Tested approximation; not replaced with accurate attribution. |
| 5. `lazy::commit` reinserts marker | Existing behavior retained and tested with failure injection. |
| 6. `resource_at` caches only success | Added helper-level HRESULT/non-null test; cache lifecycle still open. |
| 7. `ReadFileEx`, poisoned find locks | Reviewed; preserved poison-tolerant lookup and added file-buffer coverage. No injected poisoned-lock test. |

### Lock ordering

Reviewed paths take `AWAY` before `PHYS_BLOCKS` / `FREED_PAGES`, and drop those two locks before freeing sparse or UMA homes. Placeholder paths acquire their region registry before `AWAY`. No direct reverse edge was identified in the reviewed paths; this is not a global lock-order proof. `LAZY` is locked during commit. Review interactions between `AWAY`, the exception handler, sparse transfer locks and COM callbacks under contention before merging. Native virtual-memory calls bypassing these hooks can race with readability checks. Several locks in memory and GPU code still use `unwrap`; poisoning is not globally addressed.

## Open work from the requested list

### Title-only memory accounting (not implemented)

The correct ledger must account for address intervals and commit/decommit deltas, not simply sum allocation requests. Title VirtualAlloc and XMem paths overlap UMA/sparse reservations and commits; sparse homes may migrate into away mappings. Avoid double-counting recommits, overlaps, mapped aliases, and the precommitted coherent window. Track ownership of the allocation independently from physical host and GPU backing. Update only after success, and support partial decommit, release, rollback and address reuse. Preserve `XCL_TITLE_MEMORY_STRICT` as an explicitly defined diagnostic policy. No new counter was added because an inaccurate counter could be worse than the existing disclosed approximation.

### MADE cache / queue ownership (not implemented)

`resource_at` caches owning COM pointers by VA + description, with no bound, device identity, or memory generation. A concurrent miss can replace a reference. Consumers borrow the pointer outside the cache lock. Eviction cannot safely be added until consumers hold a scoped reference. Proposed design: device + backing-generation identity; invalidate on successful remap/free/heap release; retain scoped COM refs while generating descriptors; transfer evicted ownership to a queue-aware late-release path. Test address reuse, concurrent miss, in-flight eviction and heap release.

`present::QUEUES` retains references; `title_textures::upload_queues` separately retains an upload queue per device. `q_release` only observes zero remaining references, which the registry itself can prevent. The presenter and candidate use queue addresses without independently owning them. Do not simply remove registry entries or guess title ownership from a returned refcount. First give all consumers explicit owning snapshots and design title/host ownership and queue retirement; release after fences. `late_release::Held.fences` is also keyed by queue address and requires retirement/generation tracking. Queue references can keep devices alive.

There is an existing snapshot/seal risk: `drain_known` gathers queues outside the HELD lock, then seals resources which may have arrived after the snapshot. A queue/resource epoch or sealed cohort and guaranteed queue registration ordering are needed. The batching change does not resolve that issue.

### Map rollback

Validation before entering the loop is stronger, but failure during a commit still leaves prior mapping changes. A transaction must stage ownership changes and copies, retain old homes until success, undo new lazy markers/commit, and restore the old mapping state on any error. Also cover scatter and null-page entries. Do not implement rollback by merely clearing `AWAY`: that can discard the only live contents.

### BCPack (measurement required; no code change)

No title or sample run was available. No mode, request count, compressed bytes or destination bytes were measured. Existing `dstorage` reporting includes BCPack refusal counters by mode, request and byte counts and placeholder-fill information. Collect these from a legitimate runtime test (`report.json`, `xcl.log`, `detailed.txt`) before diagnosing a launch blocker. Preserve existing placeholder behavior as a diagnostic, not a decoder; it can still report `E_DECOMPRESSION`. No guessed decoder, fake success, or game-specific protection changes.

## Report-only review of remaining subsystems

These are static candidates for investigation, not confirmed runtime defects. The review was targeted rather than exhaustive; no claim of a complete audit of every file.

| Area | Finding / next verification |
|---|---|
| `sparse.rs` | Free-pool retirement needs a completed fence and elapsed grace interval; the collection path is driven by decommit, so idle pools may remain after the last decommit. Wait bookkeeping keyed by queue address needs lifecycle review. `clear_sent_range`, `readback_now` and merge arithmetic rely on bounded callers; audit all range callers before changing them. |
| `vram.rs` | `Arenas::take` triggers sweep on new placements, not periodically. Tile pools may remain allocated when entirely idle. Device-keyed `PAGES` persists. Counters such as `MADE_BYTES` are cumulative, not a live-memory estimate. Require GPU lifetime proof before reclaiming heaps. |
| `uma.rs` | `free_commit` uses a large fallback if `GlobalMemoryStatusEx` fails; test failure behavior. Coherent and sparse representations can increase host commit together. The coherent-heap precondition is roughly 17 GiB available commit in source, not measured peak usage. Existing allocator checks are useful but not proof of all lifetimes. |
| `title_textures.rs` | Per-device upload-queue ownership and table/shadow cleanup need review on device recreation. `read_committed` allocates by range length, potentially adding peak CPU staging memory. Diagnostic `SEEN` tracking may grow; verify frequency and bounds. |
| `list_x.rs` | `QUEUE_IDS` grows monotonically, while queue release removes live tracking separately. Pointer reuse could inherit prior ID/graph state. Audit the queue graph and its raw-pointer keys before assuming deadlock prevention is complete. Some copy-range arithmetic depends on earlier range checks. |
| `bind.rs`, `console_list.rs` | Diagnostic seen collections and raw-pointer layout caches are candidates for lifetime and growth checks. No confirmed bug or code change in this pass. |
| `loader.rs`, `pe.rs` | The loader can hold both full input bytes and a committed mapped image, increasing peak commit. Signed relocation-delta arithmetic and export ordinal addition warrant malformed-input boundary tests. These are report-only, not modifications to loading or protection behavior. |
| Recompiler / RDNA2 / audio / GameInput / XGameRuntime / remaining VFS | Not comprehensively audited. Prior tests do not establish real GPU or retail-title compatibility. No new code in these subsystems. |

### 32 GB system and large commit workloads

Physical RAM capacity alone does not establish sufficient system commit. Simultaneous coherent backing, sparse CPU copies, graphics staging, shader compiler/cache, WDDM allocations and image loading may exceed physical RAM. Page-file space and competing process commit matter. Preflight now reports insufficient headroom; it does not guarantee a 32 GB PC will succeed. Capture system commit, process private bytes, local/non-local GPU budgets, queue backlog, shader cache size, texture pool stats and actual BCPack counts through loading. No memory or FPS measurement was made.

## Next validation gate

Upload both archives to repository root, preserving paths. Do not delete working source files or commit game assets. Run **Patch 18 validation and release** on the uploaded branch. Obtain `XCL-Patch18-logs` even on failure; a successful run also offers `XCL-Patch18-Windows`. Capture real test summaries and internal hardware skips, and fix any compiler errors before further speculative changes. Test real hardware on an authorized machine. The CI runner has no dedicated GPU and does not establish retail-title launch, rendering correctness or 1080p performance.

**AI-use:** GPT-6.2. No claim of human verification, successful compilation, or successful gameplay.
