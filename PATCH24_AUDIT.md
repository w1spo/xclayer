# XCLayer Patch 24 — Shader lookup and diagnostic accounting

Baseline: supplied XCLayer_Patch23_FullSource.zip (194 files). Last supplied successful Windows CI was Patch 19; Patch 20–24 Windows builds and GPU runs are not established here. No game data, protection changes, dependencies, generated files or changes to supported shader ABI.

## Cumulative comparison before Patch 24
Compared original xcl-main.zip, ZIP comment 09940bb87fa11bc394fd9e5de570c2b7cfd16323, to the supplied Patch 23 archive, by relative path and file bytes. Rust line counts use splitlines; insertion/deletion counts use git diff --no-index --numstat for changed shared files and count all lines of newly added files.
- Entire repository: 186 -> 194 files; 33 original files differ, 8 added, none removed.
- All Rust: 123 -> 124 files, 32 existing files modified (26.02%), one new file. 102,428 -> 105,112 lines. 3,219 insertions, 535 deletions; churn / original lines 3.67%, net growth 2.62%.
- Core crates/xcl/src Rust: 105 -> 106 files, 32 existing files modified (30.48%), one new file. 94,449 -> 97,133 lines; churn 3.97%, net growth 2.84%.
- These figures describe source differences, not game compatibility or speed. Earlier upstream archive with another revision is not the chosen 09940bb baseline.

## 24.01 — Recompiler: reuse ranked signatures for attribute and export lookup
Root cause: vertex exports ranked their outputs, then fallback param lookup and element lookup ranked the same outputs again; mesh exports ranked twice. Pixel attribute/slot fallback also ranked twice. Factor lookup helpers accepting the already-ranked slice, preserve user-before-clip order, stable packed-element order and deduplication across both groups. Vertex explicit console tables retain priority. No shader-cache key changes, emitted shader changes intended to be zero. File: gpu/recomp/mod.rs (+33/-5), gpu/recomp/emit.rs (+8/-3). Test: ranked_lookup_preserves_packed_and_clip_register_order, including shared registers across user/clip elements and missing registers. Existing attribute tests unchanged. Not tested: Rust build, byte-identical generated HLSL across retail corpus, GPU results, actual translation latency. AI use: OpenAI ChatGPT GPT-6.2.

## 24.02 — kernelx: maintain incremental committed-memory totals
Root cause: the partial diagnostic ledger snapshot walks every reservation and committed interval while holding the global ledger mutex. Keep bytes per reservation and total bytes in Ledger; commit/decommit account for only changed intervals; contained recommits avoid splicing. Account for replacement and release. Snapshot is now O(1) in reservation/interval count. The Windows operation and lock ordering are unchanged. No change to TitleMemoryStatus or completeness of accounting. Allocation paths gain a sum of the touched intervals; benefits and tradeoffs need profiling. File: hle/kernelx/accounting.rs (+55/-6). Test: incremental_total_matches_interval_scan_after_each_operation, multiple reservations, disjoint/overlapping/contained commits, partial/full decommits, replacement and release. Existing ledger test unchanged. Not tested: Windows allocation paths, latency and contention under actual workload. AI use: OpenAI ChatGPT GPT-6.2.

## 24.03 — CI/docs
Retain existing workflow filename; set display name and log/artifact names to Patch 24. Keep existing exclusions; README documents the two changes. Files: .github/workflows/patch18-build.yml (+10/-10), README.md (+2/-0). AI use: OpenAI ChatGPT GPT-6.2.

## Validation
- git diff --check: passed.
- Three logical patches independently applied in sequence to a clean Patch 23 extraction using git apply --check and git apply. diff -qr matched the working tree before this audit was added.
- YAML parsed successfully; workflow name Patch 24 validation and release.
- cargo build --workspace --locked: exit 127 (Cargo unavailable).
- cargo test --workspace -- --test-threads=4: exit 127 (Cargo unavailable). Zero Rust tests executed, no pass/fail/ignored/skipped counts available.
- gh pr list -R ivall/xcl --search 'shader signature ranking memory ledger': exit 127 (gh unavailable); upstream overlap not verified.
- Implementation and configuration changes excluding this audit: 108 insertions / 24 deletions across five files. Two new tests. Largest logical change 61 lines. No remote push or PR performed.
- No Windows SDK or real GPU in this environment. No performance benchmarks or gameplay verification.

## Next steps
Validate Patch 23/24 CI, then compare real hardware with the last CI-passing baseline on a supported, lawfully obtained title or test sample. Record GPU frame times, upload fence waits, shader translation wall time, memory commit and VRAM peaks. Texture upload queue dependencies, Non-UMA coherence, BCPack, cache retirement and full mesh shader ABI remain unresolved. Do not infer an FPS or launch probability from source churn. Do not remove fence waits without correct dependency/lifetime design.
