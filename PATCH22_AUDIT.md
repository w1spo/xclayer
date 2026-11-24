# XCLayer Patch 22 — Device-local arenas, sparse placement coverage, write-watch validation

Baseline: the two Patch 21 archives supplied in the conversation, combined without altering their paths. Patch 19 is the last reported successful Windows CI checkpoint available here; Patch 20/21/22 runtime and compile results are not established by that log.

## 22.01 — D3D12.X: separate fallback VRAM arenas by device
Root cause: the global ARENAS allocator matched heap ranges by address, size and alignment only. A second D3D12 device could therefore receive a heap created on the first device. Add a device key to each arena and require a match when reusing it, following the existing per-device PAGES pattern. Arena IDs remain globally unique for existing release bookkeeping.
File: crates/xcl/src/hle/d3d12x/vram.rs (+22/-8).
Regression: arenas_are_not_shared_between_devices (two synthetic device keys at the same address get distinct heaps; subsequent placements on device one reuse its own heap). Existing tests updated only for the additional parameter, with assertions preserved.
Not tested: two real devices, COM identity through alternate interface pointers, address reuse across device destruction, actual memory usage. This is not complete device retirement or cache eviction.
AI use: OpenAI ChatGPT, GPT-6.2.

## 22.02 — D3D12.X: reject incomplete sparse buffer placements
Root cause: place_buffer checked only the first and last sparse windows and collected mapped tiles with filter_map. A gap or failed heap allocation could omit tiles while reporting successful placement. Validate the whole range using the existing contiguous-window check, validate padded width, require every tile, and abandon the new reserved resource before mapping it if coverage is incomplete or the transfer path failed. Drop the state lock before releasing the resource, following the existing vram placement failure pattern. CPU holes inside a tile remain supported.
File: crates/xcl/src/hle/d3d12x/sparse.rs (+31/-5).
Regression: a_placed_buffer_requires_every_tile (missing middle tile, complete partial-last-tile range, zero and misaligned spans). Existing readback range test also covers contiguous-window/gap validation used by this path.
Not tested: real VRAM exhaustion, COM release callback behaviour, partial window mapping and cleanup, caller fallback behaviour on a quarantined transfer path. Earlier window tiles are still tracked, not rolled back here; final placement signal failure policy is unchanged. This does not solve all sparse mapping transactions, residency or non-UMA coherence.
AI use: OpenAI ChatGPT, GPT-6.2.

## 22.03 — D3D12.X: validate texture write-watch spans
Root cause: unchecked source + size + alignment and base + region size arithmetic could wrap or panic, while malformed ranges could influence range merging. A full GetWriteWatch buffer with an overflowing or nonadvancing next address could fail to progress. Validate aligned watch spans, skip invalid ranges and reject invalid next cursors/counts, and guard dirty-page overlap arithmetic. No change to valid range merging or retry limits.
File: crates/xcl/src/hle/d3d12x/title_textures.rs (+32/-6).
Regression: write_watch_rejects_invalid_spans (unaligned and aligned ranges, zero size, overflow before and during alignment, malformed wanted/region ranges).
Not tested: live GetWriteWatch, concurrent remapping, actual upload performance, all texture reads and lifetimes. Invalid spans are not repaired. Other code using raw title memory still requires separate lifetime review.
AI use: OpenAI ChatGPT, GPT-6.2.

## 22.04 — CI and documentation
Update existing workflow display name and output filenames to Patch 22, preserving its filename and existing test exclusion. Add README note. No dependencies, generated code, protection mechanisms or game data changed. No upstream PR or remote push performed.
Files: .github/workflows/patch18-build.yml, README.md (+12/-10).
AI use: OpenAI ChatGPT, GPT-6.2.

## Tested / not tested
Linux environment, no Cargo, Windows SDK or real GPU execution. Commands attempted:
- `cargo build --workspace --locked`: exit 127 (cargo unavailable).
- `cargo test --workspace --locked -- --test-threads=4`: exit 127 (cargo unavailable).
- `cargo build --release --workspace --locked`: exit 127 (cargo unavailable).
- `gh pr list -R ivall/xcl --search 'sparse placed buffers device arenas texture write watch'`: exit 127 (gh unavailable). Upstream overlap not checked.
- `git diff --check`: passed.
- Each logical diff passed `git apply --check`, applied sequentially to an independent baseline tree, and `diff -qr --exclude=.git` found no differences against the edited source before adding this audit.
- Workflow YAML parsed successfully.
- Source changes before this audit: 97 insertions, 29 deletions, five files, three new tests. Largest runtime logical patch: 38 changed lines.
No Rust tests started; zero executed, pass/fail/ignored/skipped counts unavailable. No GPU smoke runs, measured FPS, verified gameplay or performance claims. Existing regression assertions and test selection not weakened.

## Open issues
Complete title memory accounting, safe MADE eviction/descriptor ownership, queue retirement, full physical mapping rollback, remaining native file I/O paths, complete non-UMA coherence, BCPack measurement/decoding and mesh shader ABI remain open. Run Windows CI, then compare real GPU results against the last validated baseline. Do not interpret static fixes as proof of retail-title compatibility.
