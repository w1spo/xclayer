# XCLayer Patch 23 — CPU hot paths and streaming allocation churn

Baseline: XCLayer_Patch22_FullSource.zip from this conversation (193 files). Last supplied successful Windows CI log was Patch 19; Patch 20–22 compilation and GPU execution have not been established here. This is a broad static performance screening of the supplied tree, not an exhaustive line-by-line review or a profiling run. Source inventory: 106 Rust files under crates/xcl/src. Reviewed selected hot paths and supporting code in DirectStorage, lazy memory, DXC and shader cache, PSO binding, late release, sparse/VRAM transfers, texture staging/write-watch/detiling, CPU writes, allocator reuse, virtual allocation ledger, VFS, logging, recompiler interface lookups, XTaskQueue and startup/PE/audio entry points. No game data or protection changes.

## 23.01 — DirectStorage: reuse bounded input staging on each worker
Root cause: execute allocated and initialized a fresh Vec for every file or memory request, then discarded it even when the next request needed the same size. Keep initialized input staging per queue worker, expose only the requested prefix, and discard capacity above 8 MiB after every request (including errors). Smaller requests do not shrink the initialized high-water mark. A successful source read fills the complete slice; an unsuccessful read exits before destination copy. The zlib output remains separately owned, with its existing size validation and padding. Keep staging for memory-to-memory copies so overlapping title source/destination is not turned into an unsafe nonoverlapping direct copy. Allocation failure remains E_OUTOFMEMORY. No new threads or parallel I/O semantics.
File: crates/xcl/src/hle/dstorage.rs (+74/-24).
Tests: input_staging_reuses_small_buffers_and_drops_oversized_ones; reused_input_staging_never_delivers_bytes_from_a_failed_read. Existing request_buffer_handles_capacity_overflow_without_aborting updated for the new buffer API with zero-length, initial zeroes and overflow assertions retained.
Not tested: Windows filesystem and native streaming timings, physical page source on real title, memory high-water per many simultaneous queues. Tradeoff: retained capacity can add up to 8 MiB per live worker, not 8 MiB process-wide. Large inputs still allocate for each request. No speedup measured.
AI use: OpenAI ChatGPT, GPT-6.2.

## 23.02 — kernelx: batch lazy-page preparation lock acquisitions
Root cause: ensure took a read lock to find marked pages and then a new write lock for each 64 KiB page. Process that same snapshot in groups of up to 64 pages under one write lock per group. Each page still uses the existing commit_marked policy and individual VirtualAlloc; if a page was unmapped before the group is processed, it is skipped, and a failed commit is reinserted. The lock is released between groups. No change to the fault handler or commit API.
File: crates/xcl/src/hle/kernelx/lazy.rs (+25/-2).
Test: batched_ensure_keeps_failures_and_skips_unmarked_pages.
Not tested: real file I/O under contention, commit pressure and exception handler tail latency. Fewer lock acquisitions trade off against longer individual lock holds; the cap is a policy, not a measured optimum. Existing allocation/unmapping race limitations are not resolved.
AI use: OpenAI ChatGPT, GPT-6.2.

## 23.03 — Recompiler: amortize DXIL flight registry pruning
Root cause: cache_flight retained all live Weak entries under a global mutex on every lookup. Return a live matching flight immediately and sweep expired entries only at a growing threshold (minimum 64 entries, then twice the live count after a sweep). Live entries retain the same Arc lock identity; stale entries may persist until a later sweep. No disk-cache key, compiler output or cross-process mutex changes.
File: crates/xcl/src/gpu/recomp/dxc.rs (+41/-9).
Test: flight_registry_amortizes_cleanup_without_losing_live_flights. Existing path alias, cross-process, same-key and independent-key tests unchanged.
Not tested: DXC concurrency on Windows, number of active flights, peak registry size or compilation throughput. The map's retained capacity follows prior high-water marks; it is not a strict fixed-size cache.
AI use: OpenAI ChatGPT, GPT-6.2.

## 23.04 — D3D12.X: avoid redundant PSO bind synchronization
Root cause: resolve_for_bind looked up the Pending entry, then resolve or resolve_wait repeated the global registry lookup. Ready compute binds also took job and done mutexes even after the nonzero real pointer was published with Release ordering. Reuse the first owning Arc, return the successful pipeline from an Acquire load, and avoid a repeated atomic RMW after note_use has been recorded. Pending or failed builds still take the existing wait path. The title must obey existing COM object lifetime rules; this does not add new lifetime guarantees for concurrently released pointers.
File: crates/xcl/src/hle/d3d12x/pso_async.rs (+36/-4).
Test: ready_compute_bind_does_not_wait_for_build_mutexes (synthetic nonzero pointer, not used as COM; holds both mutexes and verifies the ready path returns; releases the locks before joining even on timeout). Existing failure/cancel tests unchanged.
Not tested: real compute/graphics submissions, publication/retirement races on a GPU, bind call frequency or FPS. The test uses a bounded scheduling timeout and may need investigation on heavily overloaded runners, not assertion weakening.
AI use: OpenAI ChatGPT, GPT-6.2.

## 23.05 — D3D12.X: stop polling deferred releases when idle
Root cause: once started, the late-release worker slept four milliseconds and acquired the global lock indefinitely, including after all resources had been retired. A condition variable now waits on the existing held-resource predicate while no backlog exists. Enqueue on the idle transition notifies the worker. Predicate and enqueue share HELD, so notify-before-wait is safe. Active polling cadence, batch sealing, fence validation, and immediate idle drain remain unchanged.
File: crates/xcl/src/hle/d3d12x/late_release.rs (+37/-1).
Test: idle_release_worker_waits_for_a_backlog (local state, checks idle blocking and notification; no synthetic resource is sent to a real drain).
Not tested: worker wake latency and CPU usage on Windows, real GPU fences, shutdown and device loss. No resource is retired earlier on the basis of a timer; fence rules unchanged.
AI use: OpenAI ChatGPT, GPT-6.2.

## 23.06 — CI and documentation
Existing workflow filename retained; display name, output log names and artifacts updated to Patch 23. Existing test exclusion retained. README describes changes. No dependency, generated code, or build-profile changes. Runtime patches can be applied independently to the supplied Patch 22 tree; no remote PR or push performed.
Files: .github/workflows/patch18-build.yml (+10/-10), README.md (+2/-0).
AI use: OpenAI ChatGPT, GPT-6.2.

## Whole-tree performance screening: unresolved candidates and next measurements
These are observations from the source, not proven bottlenecks for a given title. Keep changes separate and require matching regression tests.

| Area | Evidence in source | Next measurement / risk |
|---|---|---|
| Texture upload | title_textures::refresh_once calls upload_all before submits; gpu::texture::upload_batch waits for its copy fence. | Time detile, staging copy and actual fence wait separately. Asynchronous uploads need title queue dependencies, state transitions, staging lifetime and failure policy; do not merely remove the wait. |
| Texture hash and detile | upload_all hashes title_bytes, then subresources_of_all obtains source bytes again. Unreadable spans may use read_committed both times. | Measure copied/hashed/detiled bytes and sparse read costs. A shared snapshot may help but can raise peak host memory; guard content changes and ownership. |
| Write-watch and texture registry | refresh_once scans cached write-watch ranges and walks all active textures on dirty or pending work; recheck_gpu_view_sources re-evaluates active and shadowed textures when mapping generation changes. | Count scans, active entries, candidates, actual uploads; a source-address index or pending set may reduce work but must support aliases and generations. |
| Sparse memory transfers | sparse::before_execute takes STATE across dirty runs, mapping and upload work; mapped_ranges sorts all mapped tile addresses when cached runs are invalidated. | Existing WATCH/PHASES counters, lock wait, upload ring occupancy, tile count. Do not unlock or retire tiles without valid lifetime proof. |
| GPU resource cache and device retirement | MADE remains unbounded and present retains title queue references; fence and uploader registries hold device-associated state. | Track live counts and VRAM across loading loops; need allocation generations, descriptor use and queue ownership before eviction. Arbitrary LRU eviction would be unsafe. |
| DirectStorage | one worker per queue; submit uses an unbounded mpsc channel; zlib creates a separate output Vec; source checks use VirtualQuery/readability checks. | Measure queue backlog, source read, inflate and physical copy throughput. Backpressure/parallel decompression must preserve ordered status, signal and cancel semantics. BCPack refused modes need real counters; no invented decoder. |
| Shader translation | recomp::ranked, param_registers and elements_at build/sort temporary signature vectors; PS attribute helpers can call them repeatedly. ShaderCache has bounded 128-entry linear lookup and returns an owned Compiled clone. | Profile per-stage translation CPU and cache hit rate; precompute immutable signature lookup only if output is byte-identical. Keep translator cache identity correct. |
| Memory ledger | accounting::alloc/free serialize native calls and maintain interval vectors, snapshot walks tracked reservations. | Measure VirtualAlloc lock contention before redesigning synchronization; ledger incomplete for GPU/UMA allocations. |
| VFS and logging | VFS has_whiteouts_under searches whiteout set; persist_whiteouts sorts/writes it; log::write serializes file output and echoes info to stderr. | Measure frequency and I/O stalls; do not buffer away crash diagnostics or change overlay semantics just to improve a benchmark. |
| Other subsystems | CPU writes worker already blocks on recv when idle; allocator reuse already tests fence completion; startup PE loader and audio/async entry points screened, not comprehensively reviewed. | Prioritize startup vs steady-state by trace; avoid broad refactors with no measured hot spot. |

Existing blockers: accurate title-only memory accounting, safe MADE cache eviction, queue retirement, overlapping/large/sparse physical rollback, remaining native file APIs, complete Non-UMA coherence, BCPack, and mesh ABI. Patch 23 is not a compatibility completion or a verified FPS improvement. Patch 19 CI is not proof Patch 23 builds.

## Tested / not tested
Environment: Linux container, no cargo, rustc, Windows SDK or hardware GPU available for this task.
- `cargo build --workspace --locked`: exit 127, cargo unavailable.
- `cargo test --workspace --locked -- --test-threads=4`: exit 127, cargo unavailable.
- `cargo build --release --workspace --locked`: exit 127, cargo unavailable.
- `gh pr list -R ivall/xcl --search 'DirectStorage staging lazy page batching DXC PSO late release'`: exit 127, gh unavailable. Upstream overlap not verified.
- `git diff --check`: passed.
- Six logical diffs (five runtime, one CI/docs) each passed `git apply --check` and applied to an independently extracted Patch 22 tree; `diff -qr --exclude=.git` found no differences before adding this audit.
- Workflow YAML parsed successfully.
- 225 insertions, 50 deletions, seven modified files excluding this audit; six newly added tests. Largest runtime logical diff: 98 changed lines. No assertion or ignore-list weakening. Zero Rust tests executed; pass/fail/ignore/skip counts unavailable. No benchmark timings or gameplay claims.
- Required next validation: run `Patch 23 validation and release` GitHub Actions. On authorized hardware and supported game/test samples, compare same settings and same run duration against baseline using `scripts/smoke.ps1 -Game <folder>` from clean commits and review detailed.txt/report.json. Record cold and warm shader cache runs separately. Report real GPU test skips and frame times, RAM/VRAM/commit peaks and correctness, not FPS alone.
