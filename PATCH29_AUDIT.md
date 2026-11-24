# Patch 29 reconstruction

Base: `d96249bc17ff971d5be3eabe2267c1f1cd187f8c` (Patch 24 commit, with Patch 25 audit present).

This is a new reconstruction from the repository, not the original Patch 29 ZIP.
It does not include or assert equivalence with missing Patches 26–28.

## Changes
- XG copy-in/copy-out preflight: checked row/slice spans and offsets, committed readable source and writable destination regions. Reject guard pages, invalid pitches and address arithmetic overflow before copying. The caller must still keep memory mappings stable through the call.
- Invalidate upload hash when shadowed tiles are restored. Conservatively refresh CPU-owned GPU-view textures on mapping-generation changes, including unchanged source addresses. This may increase upload work during frequent mapping changes.
- Use one owned texture-memory snapshot for hashing and detiling; retain existing batch/parallel detiling path. This increases temporary memory and copying relative to the borrowed-memory path. Coherent reads use the existing fault-tolerant copy helper.
- Add pitch boundary regression test.

## Validation and limitations
No Rust build, tests, Windows GPU execution, or performance benchmark was run in this environment. Existing workflow remains unchanged; run Windows CI and hardware tests before merging.
The original archive is unavailable. GPU-view mapping cleanup when releasing a texture pool is not implemented here, and the existing remap failure/queue lifetime policy is unchanged. No assertion of complete non-UMA coherence, game compatibility or FPS.
