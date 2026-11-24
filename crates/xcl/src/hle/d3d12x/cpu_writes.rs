//! Write32/64BitValueBottomOfPipeX into memory the GPU does not see: done by the CPU once the GPU
//! has finished the work before them.
//!
//! A console title marks its GPU progress with bottom-of-pipe writes into its own memory and
//! waits on them (a retail game: plain `VirtualAlloc` memory, outside XCL's unified memory).
//! The PC GPU cannot write there; dropped, the title waited for ever after its second frame.
//! Here each queue gets a fence of XCL's: after a submission whose lists carry such writes the
//! queue signals it, and a worker thread does the writes, in submission order, when the GPU is
//! there – what bottom of pipe means.

use std::collections::{HashMap, VecDeque};
use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{mpsc, Mutex, OnceLock};

use windows::core::Interface;
use windows::Win32::Graphics::Direct3D12::*;
use std::time::{Duration, Instant};

/// A bottom-of-pipe write into memory outside unified memory, applied by the CPU once the
/// queue's fence reaches the submission that recorded it.
#[derive(Clone)]
pub(super) enum Write {
    /// Immediate dwords to store starting at `va`.
    Immediate(usize, Vec<u32>),
    /// A 64-bit value resolved by the GPU into a persistently mapped READBACK buffer slot
    /// (`src`), copied atomically to `va` once the queue's fence completes. Holding `_buf` keeps
    /// the mapping alive until the worker finishes.
    ReadbackU64 { va: usize, src: usize, _buf: ID3D12Resource },
}

struct Job {
    fence: ID3D12Fence,
    value: u64,
    writes: Vec<Write>,
    enqueued: Instant,
    warned: bool,
}

// SAFETY: the fence is a free-threaded COM object; the writes are plain data
unsafe impl Send for Job {}

/// queue → its fence and the last value signalled
struct QueueFence {
    fence: ID3D12Fence,
    value: u64,
    failed: bool,
}

unsafe impl Send for QueueFence {}

static FENCES: Mutex<Option<HashMap<usize, QueueFence>>> = Mutex::new(None);

/// A timeout is diagnostic only; it is never evidence of GPU completion.
const WAIT_MS: u64 = 10_000;
const POLL_MS: u64 = 5;

#[derive(Debug, PartialEq, Eq)]
enum FenceState { Pending, Complete, Removed }

fn fence_state(completed: u64, target: u64) -> FenceState {
    if completed == u64::MAX { FenceState::Removed }
    else if super::fence_x::lifetime_complete(completed, target) { FenceState::Complete }
    else { FenceState::Pending }
}

/// If completion cannot be proved, retain mapped readback buffers until exit.
/// Never write a speculative completion marker to the title's memory.
fn quarantine(writes: Vec<Write>, reason: &str) {
    static FAILURES: AtomicU64 = AtomicU64::new(0);
    let n = FAILURES.fetch_add(1, Ordering::Relaxed) + 1;
    if n.is_power_of_two() {
        crate::xlog_error!("D3D12.X: bottom-of-pipe writes quarantined ({reason}, {n} failed batches; no completion written)");
        crate::log::detail(format_args!("cpu_writes_quarantine reason={reason} batches={n}"));
    }
    for w in writes {
        if let Write::ReadbackU64 { _buf, .. } = w { std::mem::forget(_buf); }
    }
}

unsafe fn service_pending(pending: &mut HashMap<usize, VecDeque<Job>>) {
    pending.retain(|_, jobs| {
        // Preserve submission order on each fence, but do not block other queues.
        while let Some(job) = jobs.front_mut() {
            let completed = job.fence.GetCompletedValue();
            match fence_state(completed, job.value) {
                FenceState::Pending => {
                    if !job.warned && job.enqueued.elapsed() >= Duration::from_millis(WAIT_MS) {
                        job.warned = true;
                        crate::xlog_warn!("D3D12.X: bottom-of-pipe writes still pending after {WAIT_MS} ms (fence at {completed}, wanted {}); no completion written", job.value);
                        crate::log::detail(format_args!("cpu_writes_timeout completed={completed} target={} queued={}", job.value, job.writes.len()));
                    }
                    break;
                }
                FenceState::Removed => {
                    let job = jobs.pop_front().unwrap();
                    quarantine(job.writes, "device_removed");
                }
                FenceState::Complete => {
                    let job = jobs.pop_front().unwrap();
                    for w in &job.writes {
                        match w {
                            Write::Immediate(va, words) => write_now(*va, words),
                            Write::ReadbackU64 { va, src, .. } => {
                                let value = (*(*src as *const AtomicU64)).load(Ordering::Acquire);
                                write_u64_now(*va, value);
                            }
                        }
                    }
                }
            }
        }
        !jobs.is_empty()
    });
}

fn enqueue(pending: &mut HashMap<usize, VecDeque<Job>>, job: Job) {
    pending.entry(job.fence.as_raw() as usize).or_default().push_back(job);
}

fn worker() -> &'static Mutex<mpsc::Sender<Job>> {
    static W: OnceLock<Mutex<mpsc::Sender<Job>>> = OnceLock::new();
    W.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<Job>();
        let spawned = std::thread::Builder::new().name("xcl-cpu-writes".into()).spawn(move || unsafe {
            let mut pending = HashMap::new();
            loop {
                // No periodic wakeups if idle. Limit the drain so constant submissions
                // cannot starve the completion checks.
                let disconnected = if pending.is_empty() {
                    match rx.recv() { Ok(job) => { enqueue(&mut pending, job); false }, Err(_) => break }
                } else {
                    match rx.recv_timeout(Duration::from_millis(POLL_MS)) {
                        Ok(job) => { enqueue(&mut pending, job); false }
                        Err(mpsc::RecvTimeoutError::Timeout) => false,
                        Err(mpsc::RecvTimeoutError::Disconnected) => true,
                    }
                };
                for _ in 0..127 {
                    match rx.try_recv() { Ok(job) => enqueue(&mut pending, job), Err(_) => break }
                }
                service_pending(&mut pending);
                if disconnected {
                    // Sender is process-global in normal use. If it disappears, do not
                    // free any still-live readback buffer or signal pending writes.
                    for (_, jobs) in pending.drain() {
                        for job in jobs { quarantine(job.writes, "worker_disconnected"); }
                    }
                    break;
                }
            }
        });
        if let Err(e) = spawned {
            crate::xlog_error!("D3D12.X: no thread for the writes after the GPU: {e}");
            crate::log::detail(format_args!("cpu_writes_worker_start_failed"));
        }
        Mutex::new(tx)
    })
}

/// Check the range before querying memory or constructing a reference to an atomic.
fn checked_write_end(va: usize, bytes: usize, alignment: usize) -> Option<usize> {
    if va == 0 || bytes == 0 || va % alignment != 0 {
        return None;
    }
    va.checked_add(bytes)
}

/// A single committed, writable region, without PAGE_GUARD. The mapping may still
/// change later: callers must keep the title allocation alive until the write finishes.
fn writable_region(va: usize, end: usize) -> bool {
    use windows_sys::Win32::System::Memory::{VirtualQuery, MEMORY_BASIC_INFORMATION, MEM_COMMIT, PAGE_EXECUTE_READWRITE, PAGE_GUARD, PAGE_READWRITE};
    let mut m: MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
    let found = unsafe { VirtualQuery(va as *const c_void, &mut m, std::mem::size_of_val(&m)) } != 0;
    found
        && m.State == MEM_COMMIT
        && m.Protect & PAGE_GUARD == 0
        && m.Protect & (PAGE_READWRITE | PAGE_EXECUTE_READWRITE) != 0
        && (m.BaseAddress as usize) <= va
        && (m.BaseAddress as usize).checked_add(m.RegionSize).is_some_and(|region_end| region_end >= end)
}

/// The dwords at `va`, if the memory is still there and writable.
unsafe fn write_now(va: usize, words: &[u32]) {
    let Some(bytes) = words.len().checked_mul(4) else { return };
    let Some(end) = checked_write_end(va, bytes, std::mem::align_of::<AtomicU32>()) else { return };
    if !writable_region(va, end) {
        return;
    }
    for (i, &w) in words.iter().enumerate() {
        (*((va + 4 * i) as *const AtomicU32)).store(w, Ordering::Release);
    }
}

/// An 8-byte aligned 64-bit value at `va`, if the memory is still there and writable.
unsafe fn write_u64_now(va: usize, value: u64) {
    let Some(end) = checked_write_end(va, 8, std::mem::align_of::<AtomicU64>()) else { return };
    if !writable_region(va, end) {
        return;
    }
    (*(va as *const AtomicU64)).store(value, Ordering::Release);
}

/// Whether `[va, va + bytes)` is memory the CPU can write for the GPU later.
pub(super) fn writable(va: usize, bytes: usize) -> bool {
    let Some(end) = checked_write_end(va, bytes, std::mem::align_of::<AtomicU32>()) else { return false };
    crate::hle::kernelx::lazy::ensure(va, bytes);
    writable_region(va, end)
}

/// `writes` of lists just submitted to `queue`: done when the GPU has finished them.
pub(super) unsafe fn after(queue: *mut c_void, writes: Vec<Write>) {
    if writes.is_empty() { return; }
    if queue.is_null() { quarantine(writes, "null_queue"); return; }
    let Some(q) = ID3D12CommandQueue::from_raw_borrowed(&queue) else {
        quarantine(writes, "invalid_queue"); return;
    };
    let mut g = FENCES.lock().unwrap();
    let m = g.get_or_insert_with(HashMap::new);
    let entry = match m.entry(queue as usize) {
        std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
        std::collections::hash_map::Entry::Vacant(v) => {
            let mut dev: Option<ID3D12Device> = None;
            if q.GetDevice(&mut dev).is_err() { quarantine(writes, "get_device"); return; }
            let Some(dev) = dev else { quarantine(writes, "no_device"); return; };
            let Ok(fence) = dev.CreateFence::<ID3D12Fence>(0, D3D12_FENCE_FLAG_NONE) else {
                quarantine(writes, "create_fence"); return;
            };
            v.insert(QueueFence { fence, value: 0, failed: false })
        }
    };
    if entry.failed { quarantine(writes, "queue_tracking_failed"); return; }
    let Some(value) = entry.value.checked_add(1).filter(|&v| v != u64::MAX) else {
        entry.failed = true;
        quarantine(writes, "fence_counter_exhausted"); return;
    };
    if q.Signal(&entry.fence, value).is_err() {
        entry.failed = true;
        quarantine(writes, "signal_failed"); return;
    }
    entry.value = value;
    let job = Job { fence: entry.fence.clone(), value, writes, enqueued: Instant::now(), warned: false };
    static FIRST: std::sync::Once = std::sync::Once::new();
    FIRST.call_once(|| crate::xlog_info!("D3D12.X: bottom-of-pipe writes outside unified memory are done by the CPU after the GPU ({} in the first submission)", job.writes.len()));
    // Keep Signal and enqueue serialized; otherwise two concurrent producers can
    // send the higher fence value first and reverse their CPU writes.
    if let Err(e) = worker().lock().unwrap().send(job) {
        entry.failed = true;
        quarantine(e.0.writes, "worker_send_failed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL_11_0;
    use windows::Win32::Graphics::Dxgi::*;

    #[test]
    fn completion_requires_real_fence_progress() {
        assert_eq!(fence_state(0, 1), FenceState::Pending);
        assert_eq!(fence_state(9, 10), FenceState::Pending);
        assert_eq!(fence_state(10, 10), FenceState::Complete);
        assert_eq!(fence_state(11, 10), FenceState::Complete);
        assert_eq!(fence_state(u64::MAX, 10), FenceState::Removed);
    }

    #[test]
    fn invalid_atomic_write_ranges_are_rejected() {
        assert_eq!(checked_write_end(0, 4, 4), None);
        assert_eq!(checked_write_end(0x1001, 4, 4), None);
        assert_eq!(checked_write_end(0x1004, 8, 8), None);
        assert_eq!(checked_write_end(0x1000, 0, 4), None);
        assert_eq!(checked_write_end(usize::MAX - 3, 4, 4), None);
        assert_eq!(checked_write_end(0x1000, 8, 8), Some(0x1008));
    }

    /// The value lands after the queue has done what came before it.
    #[test]
    fn writes_land_after_the_queue() {
        unsafe {
            let mut dev: Option<ID3D12Device> = None;
            if D3D12CreateDevice(None, D3D_FEATURE_LEVEL_11_0, &mut dev).is_err() {
                let factory: IDXGIFactory4 = CreateDXGIFactory1().unwrap();
                let warp: IDXGIAdapter = factory.EnumWarpAdapter().unwrap();
                D3D12CreateDevice(&warp, D3D_FEATURE_LEVEL_11_0, &mut dev).expect("WARP D3D12 device");
            }
            let dev = dev.unwrap();
            let q: ID3D12CommandQueue = dev.CreateCommandQueue(&D3D12_COMMAND_QUEUE_DESC { Type: D3D12_COMMAND_LIST_TYPE_DIRECT, ..Default::default() }).unwrap();
            let cell = Box::new([0u32; 2]);
            let va = cell.as_ptr() as usize;
            assert!(writable(va, 8));
            after(q.as_raw(), vec![Write::Immediate(va, vec![7, 9])]);
            let t = std::time::Instant::now();
            while (*(va as *const AtomicU32)).load(Ordering::Acquire) != 7 {
                assert!(t.elapsed().as_secs() < 5, "never written");
                std::thread::yield_now();
            }
            assert_eq!(*cell, [7, 9]);
        }
    }

    /// A ReadbackU64 write copies the 64-bit value from the mapped readback buffer after the queue finishes.
    #[test]
    fn readback_u64_lands_after_the_queue() {
        unsafe {
            let mut dev: Option<ID3D12Device> = None;
            if D3D12CreateDevice(None, D3D_FEATURE_LEVEL_11_0, &mut dev).is_err() {
                let Ok(factory) = CreateDXGIFactory1::<IDXGIFactory4>() else {
                    crate::gpu::recomp::runner::skip_test("no GPU");
                    return;
                };
                let Ok(warp) = factory.EnumWarpAdapter::<IDXGIAdapter>() else {
                    crate::gpu::recomp::runner::skip_test("no GPU");
                    return;
                };
                if D3D12CreateDevice(&warp, D3D_FEATURE_LEVEL_11_0, &mut dev).is_err() {
                    crate::gpu::recomp::runner::skip_test("no GPU");
                    return;
                }
            }
            let dev = dev.unwrap();
            let q: ID3D12CommandQueue = dev.CreateCommandQueue(&D3D12_COMMAND_QUEUE_DESC { Type: D3D12_COMMAND_LIST_TYPE_DIRECT, ..Default::default() }).unwrap();
            let hp = D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_READBACK, ..Default::default() };
            let rd = D3D12_RESOURCE_DESC {
                Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
                Width: 8,
                Height: 1,
                DepthOrArraySize: 1,
                MipLevels: 1,
                SampleDesc: windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
                ..Default::default()
            };
            let mut rb: Option<ID3D12Resource> = None;
            dev.CreateCommittedResource(&hp, D3D12_HEAP_FLAG_NONE, &rd, D3D12_RESOURCE_STATE_COPY_DEST, None, &mut rb).unwrap();
            let rb = rb.unwrap();
            let mut ptr: *mut c_void = std::ptr::null_mut();
            rb.Map(0, None, Some(&mut ptr)).unwrap();
            let expected = 0x1234_5678_9ABC_DEF0u64;
            (*(ptr as *const AtomicU64)).store(expected, Ordering::Release);

            let dst = Box::new(0u64);
            let va = dst.as_ref() as *const u64 as usize;
            assert!(writable(va, 8));
            after(q.as_raw(), vec![Write::ReadbackU64 { va, src: ptr as usize, _buf: rb }]);
            let t = std::time::Instant::now();
            while (*(va as *const AtomicU64)).load(Ordering::Acquire) != expected {
                assert!(t.elapsed().as_secs() < 5, "readback value never written");
                std::thread::yield_now();
            }
            assert_eq!(*dst, expected);
        }
    }
}
