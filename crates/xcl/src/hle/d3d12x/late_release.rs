//! The title's last `Release` of a resource, put off until the GPU is past the work in flight.
//!
//! D3D12 on the PC asks that an object outlives the GPU work that uses it. A console title
//! keeps to that by its own count of finished frames – but XCL's submits are not the title's:
//! texture uploads go out with a later submit, tile mappings on a queue of XCL's own. A retail
//! title's final `Release` of a resource then came while a queue still had work on it (debug
//! layer: "CORRUPTION: … is referenced by GPU operations in-flight", id 921).
//!
//! The last reference is held here instead. Each batch of held resources gets a fence value
//! signalled on every queue with work on it; the batch goes once all of them have passed it.
use std::collections::{HashMap, VecDeque};
use std::ffi::c_void;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use windows::core::Interface;
use windows::Win32::Graphics::Direct3D12::*;

/// a batch is not closed more often than this (a `Signal` a queue each time)
const SEAL_EVERY: Duration = Duration::from_millis(10);
/// how often the thread here looks for batches that can go
const TICK: Duration = Duration::from_millis(4);

struct Batch {
    dead: Vec<usize>,
    /// (fence, value) a queue: all reached = nothing submitted before the batch closed is left
    marks: Vec<(ID3D12Fence, u64)>,
}

#[derive(Default)]
struct Held {
    /// queue → its fence here and the last value signalled on it
    fences: HashMap<usize, (ID3D12Fence, u64)>,
    open: Vec<usize>,
    sealed: VecDeque<Batch>,
    last_seal: Option<Instant>,
    seal_generation: u64,
}

// (the interface pointers are only used under the lock)
unsafe impl Send for Held {}

impl Held {
    fn is_empty(&self) -> bool {
        self.open.is_empty() && self.sealed.is_empty()
    }

    /// Only the first release in a backlog needs a synchronous drain attempt.
    fn enqueue(&mut self, res: usize) -> bool {
        let idle = self.is_empty();
        self.open.push(res);
        idle
    }

    /// Close the open batch (at most every `SEAL_EVERY`, or `seal_now`) with a fence value on each of `queues`,
    /// and take out the batches every queue is past: the resources to release now.
    #[cfg(test)]
    unsafe fn drain(&mut self, queues: &[ID3D12CommandQueue], seal_now: bool) -> Vec<usize> {
        self.drain_cohort(queues, seal_now, self.seal_generation, self.open.len())
    }

    /// Only the prefix that existed before the queue snapshot may be sealed with it.
    /// Another drain that already sealed that prefix invalidates this snapshot.
    unsafe fn drain_cohort(&mut self, queues: &[ID3D12CommandQueue], seal_now: bool, generation: u64, count: usize) -> Vec<usize> {
        if generation == self.seal_generation && count != 0 && count <= self.open.len()
            && (seal_now || self.last_seal.is_none_or(|t| t.elapsed() >= SEAL_EVERY)) {
            let mut marks = Vec::with_capacity(queues.len());
            let mut ok = true;
            for q in queues {
                let key = q.as_raw() as usize;
                if !self.fences.contains_key(&key) {
                    let mut dev: Option<ID3D12Device> = None;
                    let made = q.GetDevice(&mut dev).ok().and(dev).and_then(|d| d.CreateFence::<ID3D12Fence>(0, D3D12_FENCE_FLAG_NONE).ok());
                    let Some(f) = made else {
                        ok = false;
                        break;
                    };
                    self.fences.insert(key, (f, 0));
                }
                let (fence, value) = self.fences.get_mut(&key).unwrap();
                if q.Signal(&*fence, *value + 1).is_err() {
                    ok = false;
                    break;
                }
                *value += 1;
                marks.push((fence.clone(), *value));
            }
            // (no fence or no signal: the device is gone – the batch stays open, and held)
            if ok {
                self.last_seal = Some(Instant::now());
                self.seal_generation = self.seal_generation.wrapping_add(1);
                let dead = self.open.drain(..count).collect();
                self.sealed.push_back(Batch { dead, marks });
            }
        }
        let mut gone = Vec::new();
        while self.sealed.front().is_some_and(|b| b.marks.iter().all(|(f, v)| super::fence_x::lifetime_complete(f.GetCompletedValue(), *v))) {
            gone.extend(self.sealed.pop_front().unwrap().dead);
        }
        gone
    }
}

static HELD: Mutex<Option<Held>> = Mutex::new(None);

/// Whether last releases are held (`XCL_LATE_RELEASE=0` turns it off).
pub fn enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| !std::env::var("XCL_LATE_RELEASE").is_ok_and(|v| v.trim() == "0"))
}

/// `res` (an adopted resource) holds one reference that is now this module's.
/// Try to release an idle batch immediately. With work outstanding, accumulate releases
/// for the worker's next seal instead of spinning or cloning the queue list per release.
pub unsafe fn hold(res: usize) {
    let idle = HELD.lock().unwrap().get_or_insert_with(Held::default).enqueue(res);
    if idle {
        drain_known(true);
    }
    if HELD.lock().unwrap().as_ref().is_none_or(Held::is_empty) {
        return;
    }
    // A thread of its own lets the held ones go: the title may release a resource and then
    // wait for its memory without another submit (an imported heap pins the pages under its
    // resources – unmapping them stood still while one was held "until the next submit").
    use std::sync::atomic::{AtomicBool, Ordering};
    static STARTED: AtomicBool = AtomicBool::new(false);
    static WARNED: AtomicBool = AtomicBool::new(false);
    if STARTED.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).is_ok() {
        if let Err(e) = std::thread::Builder::new().name("xcl-late-release".into()).spawn(|| loop {
            std::thread::sleep(TICK);
            unsafe { drain_known(false) };
        }) {
            // The next hold() can retry. Never release resources early to compensate.
            STARTED.store(false, Ordering::Release);
            if !WARNED.swap(true, Ordering::Relaxed) {
                crate::xlog_warn!("D3D12.X: late-release worker could not start: {e}");
                crate::log::detail(format_args!("late_release_worker_start_failed"));
            }
        }
    }
}

/// Release what every queue XCL knows of is past: the title's, the tile-mapping one, the
/// upload ones.
pub unsafe fn drain_known(seal_now: bool) {
    let (generation, count) = {
        let g = HELD.lock().unwrap();
        let Some(h) = g.as_ref().filter(|h| !h.is_empty()) else { return };
        (h.seal_generation, h.open.len())
    };
    let mut queues = super::present::known_queues();
    queues.extend(super::sparse::queue());
    queues.extend(super::title_textures::upload_queues());
    queues.extend(super::vram::mapping_queues());
    let gone = match HELD.lock().unwrap().as_mut() {
        Some(h) => h.drain_cohort(&queues, seal_now, generation, count),
        None => return,
    };
    // (outside the lock: a release may end other objects, whose releases come back here)
    for res in gone {
        // The title's last Release may precede GPU completion. Only now is the
        // page/arena lifetime eligible for retirement; keep the COM ref until cleanup.
        super::vram::forget(res);
        let f: unsafe extern "system" fn(*mut c_void) -> u32 = std::mem::transmute(super::real::orig_fn(res as *mut c_void, 2));
        f(res as *mut c_void);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC;

    /// A held resource stays while a queue has not got past the work submitted before it was
    /// released, and goes once the queue has.
    #[test]
    fn a_held_resource_goes_when_the_queue_is_past_its_batch() {
        unsafe {
            let Ok(dev) = crate::gpu::agility::create_device(None, windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL_11_0) else {
                return crate::gpu::recomp::runner::skip_test("no GPU");
            };
            let queue: ID3D12CommandQueue = dev.CreateCommandQueue(&D3D12_COMMAND_QUEUE_DESC { Type: D3D12_COMMAND_LIST_TYPE_DIRECT, ..Default::default() }).unwrap();
            // the queue stands still, as with work in flight: it waits for a fence nobody has signalled
            let gate: ID3D12Fence = dev.CreateFence(0, D3D12_FENCE_FLAG_NONE).unwrap();
            queue.Wait(&gate, 1).unwrap();
            let mut res: Option<ID3D12Resource> = None;
            let desc = D3D12_RESOURCE_DESC {
                Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
                Width: 4096,
                Height: 1,
                DepthOrArraySize: 1,
                MipLevels: 1,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
                ..Default::default()
            };
            dev.CreateCommittedResource(&D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_DEFAULT, ..Default::default() }, D3D12_HEAP_FLAG_NONE, &desc, D3D12_RESOURCE_STATE_COMMON, None, &mut res).unwrap();
            let raw = res.unwrap().into_raw() as usize;

            let mut held = Held::default();
            held.open.push(raw);
            let queues = [queue.clone()];
            assert!(held.drain(&queues, false).is_empty(), "the queue has not passed the batch's fence value");
            std::thread::sleep(Duration::from_millis(100));
            assert!(held.drain(&queues, false).is_empty(), "still in flight: still held");
            assert!(!held.is_empty());

            gate.Signal(1).unwrap();
            let start = Instant::now();
            let mut gone = Vec::new();
            while gone.is_empty() && start.elapsed() < Duration::from_secs(10) {
                std::thread::sleep(Duration::from_millis(5));
                gone = held.drain(&queues, false);
            }
            assert_eq!(gone, vec![raw], "released once the queue is past the batch");
            assert!(held.is_empty());
            drop(ID3D12Resource::from_raw(raw as *mut c_void));
        }
    }

    #[test]
    fn only_the_first_release_in_a_backlog_requests_an_immediate_seal() {
        let mut held = Held::default();
        assert!(held.enqueue(1));
        assert!(!held.enqueue(2));
        assert!(!held.enqueue(3));
        assert_eq!(unsafe { held.drain(&[], true) }, vec![1, 2, 3]);
        assert!(held.enqueue(4));
    }

    #[test]
    fn queue_snapshot_only_seals_its_original_cohort() {
        let mut h = Held::default();
        h.enqueue(1);
        let generation = h.seal_generation;
        let count = h.open.len();
        h.enqueue(2);
        assert_eq!(unsafe { h.drain_cohort(&[], true, generation, count) }, vec![1]);
        assert!(unsafe { h.drain_cohort(&[], true, generation, count) }.is_empty());
        assert_eq!(h.open, vec![2]);
        assert_eq!(unsafe { h.drain(&[], true) }, vec![2]);
    }

    /// With no queue at all nothing can be in flight: the next drain lets the batch go.
    #[test]
    fn without_queues_a_batch_goes_at_once() {
        let mut held = Held::default();
        held.open.push(0x1000);
        assert_eq!(unsafe { held.drain(&[], false) }, vec![0x1000]);
        assert!(held.is_empty());
    }
}
