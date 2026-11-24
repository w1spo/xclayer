# XCLayer – Patch 17 audit (static review, NOT compiled, NOT run)

Scope: static read of `XCL_Patch16_GitHub_Part_01/02`. There was no Rust toolchain and no Windows
in the review environment, so **nothing here was built or tested**. Run
`cargo build --workspace` and `cargo test --workspace -- --test-threads=4` before trusting any of it.

Reviewed in depth: `hle/kernelx.rs` (memory, physical pages, TitleMemoryStatus), `kernelx/lazy.rs`,
`kernelx/placeholders.rs`, `d3d12x/late_release.rs`, `d3d12x/frame.rs`, `d3d12x/cpu_writes.rs`,
`d3d12x/real.rs` (global maps, release path, `resource_at`), `d3d12x/device_x.rs` (depth-bias cache),
`hle/dstorage.rs` (requests, execute), `vfs_hooks.rs` (file + find hooks), `tls.rs`, `eh.rs` (panic scan),
`preflight.rs`, `hooks.rs`, `host.rs`.

NOT reviewed: the recompiler (`gpu/recomp/*`, `rdna2*`), `sparse.rs`, `vram.rs`, `uma.rs`,
`title_textures.rs`, `list_x.rs`, `bind.rs`, `console_list.rs`, `loader.rs`, `pe.rs`, `vfs.rs`,
XAudio2 / XMA2 / Opus, GameInput, XGameRuntime.

## Fixed in this patch (7 changes, all small, all uncompiled)

| # | File | Problem | Fix |
|---|------|---------|-----|
| 1 | `hle/kernelx.rs` `unmap_away` | Copies a physical page back home from the address it was mapped at, without checking the address is still committed. If the title decommitted or released that range, XCL itself takes an access violation the next time the page is mapped elsewhere. | Copy only if `committed_page(at)`. |
| 2 | `hle/kernelx.rs` `VirtualFree` | Releasing a reservation with physical pages mapped into it left stale entries in `AWAY` (page→address) and in `lazy::LAZY`. Stale `LAZY` entries let the AV handler / `ensure()` commit (`PAGE_READWRITE`) pages that a later allocation now owns. | New `unmap_pages_in` / `release_mapped_pages`: pages mapped in a range go home before it is released (sparse and plain paths). One lock when nothing is mapped. |
| 3 | `hle/kernelx.rs` `FreeTitlePhysicalPages` | Per-block counter only: freeing one page twice drained the block and freed its home while other pages were live. | `FREED_PAGES` set; a repeat free is ignored; a block's entries go with the block. |
| 4 | `hle/kernelx.rs` `TitleMemoryStatus` | `used = min(process private bytes, 13.5 GiB)`. The process commits 15-19 GB in menus (WDDM video memory, DXC, caches), so `title_avail` was **0**. A streaming title sizing pools from it shrinks or refuses to allocate. | Cap `used` at 75 % of the budget. `XCL_TITLE_MEMORY_STRICT=1` restores the old figure. Logged once. |
| 5 | `kernelx/lazy.rs` `commit` | The page was removed from `LAZY` before `VirtualAlloc`; on commit failure it was neither lazy nor committed. | Re-insert on failure. |
| 6 | `d3d12x/real.rs` `resource_at` | A failed placed-resource creation was cached **forever** (null), so a view made before its memory was backed stayed empty for the whole run. | Cache only successes. |
| 7 | `vfs_hooks.rs` | `ReadFileEx` not hooked: a buffer in a lazily mapped page failed with ERROR_NOACCESS (only `ReadFile` was covered). `FindClose` / `is_merged` fell through to the real `FindClose` with a heap pointer if the lock was poisoned. | `ReadFileEx` hook (non-fatal if hooking fails); poison-tolerant locks. |

No tests were added (cannot run them). Each fix needs one; see the GPT prompt.

## Findings NOT fixed (ranked for a GTA-class title)

**Blockers (need real work, not a patch)**
1. **BCPack is refused** (`dstorage.rs` `Request::check` returns `E_DECOMPRESSION` for any BCPack mode; swizzle modes too).
   Console titles that stream BCPack textures cannot load them. `XCL_*` BCPack fill switch only writes solid placeholder blocks.
   Check `report.json` for the BCPack refusal counters first – if they are non-zero this is the launch blocker.
2. **Retail-game state per `HANDOFF.md`**: "gameplay not reached", minimap mesh pipelines refused, ~4.8 GB of title
   `VirtualAlloc` memory invisible to the GPU (empty V#s), buffer views outside unified memory get an empty V#, rect lists.
3. **System commit**: the title needs ~15-19 GB commit plus DXC; `preflight.rs` never checks commit limit / page file.
   Add a preflight warning (e.g. `K32GetPerformanceInfo`: `CommitLimit - CommitTotal`).

**Memory / leak items (lower severity)**
4. `real.rs` `MADE` (view→placed resource) grows without bound and is never invalidated when the title remaps or frees the
   backing memory (stale placement risk for streaming). Needs an eviction + invalidation design.
5. `present.rs` `QUEUES`: queues are AddRef'd and never released (leak per queue; keeps a device alive across device recreation).
6. `late_release::hold` spins up to 300 us per last `Release` and calls `drain_known` (clones every queue) on each spin:
   thousands of releases at a level load = visible stall. Batch it.
7. `map_pages` (`kernelx.rs`) has no rollback when a mid-loop `VirtualAlloc` fails: earlier pages stay mapped.
8. `XMemAllocDefault` graphics fallback uses plain `VirtualAlloc` and ignores alignments above 64 KiB (4 MB pages).
9. `ReadFileScatter`, `WriteFile` from never-touched lazy pages, and `Nt*` file calls are not covered by the lazy-commit / VFS hooks.
10. `d3d12x/null.rs` leaks per `CreateDescriptorHeap` / blob (stub device only; not on the real path).
11. Failure-only `mem::forget` quarantines (`cpu_writes.rs`, `texture.rs`) are intentional, bounded by device failures.

**Verified OK (no change)**: lazy-page race (write lock is held across commit), late-release fence logic, frame-clock vectors
(bounded), DirectStorage vtables (one-time `OnceLock`), depth-bias PSO cache (capped, released), TLS slot exchange, exception
handler (poison-tolerant), VFS merged find handles.

## About GTA 6
I have no facts about the game's binary, protection or engine beyond what the repo says. If its executable is encrypted /
packed / tamper-protected, the PE loader will not get past it and that is outside what this project (or I) should work on.
Run with `XCL_NETWORK=0`, `XCL_TRACE_MEM=200`, `XCL_ACTIVITY=1`, and send `report.json` + the log: the first failing
call tells us more than any static review.
