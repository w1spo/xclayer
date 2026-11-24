//! Accounting of explicit plain Windows virtual allocations made through kernelx.
//! This is a diagnostic subset, not the process commit or the entire console budget.
use std::collections::BTreeMap;
use std::ffi::c_void;
use std::sync::Mutex;
use windows_sys::Win32::Foundation::{GetLastError, SetLastError};
use windows_sys::Win32::System::Memory as mem;

// Supported Windows x64 hosts use 4 KiB virtual memory pages, not console 64 KiB pages.
const PAGE: usize = 4096;

struct Reservation {
    end: usize,
    /// Sorted, disjoint committed intervals. End addresses are exclusive.
    committed: Vec<(usize, usize)>,
}

impl Reservation {
    fn commit(&mut self, start: usize, end: usize) {
        if start >= end { return; }
        let first = self.committed.partition_point(|&(_, b)| b < start);
        let last = self.committed.partition_point(|&(a, _)| a <= end);
        let lo = self.committed.get(first).filter(|_| first < last).map_or(start, |&(a, _)| a.min(start));
        let hi = if first < last { self.committed[last - 1].1.max(end) } else { end };
        self.committed.splice(first..last, [(lo, hi)]);
    }

    fn decommit(&mut self, start: usize, end: usize) {
        if start >= end { return; }
        let first = self.committed.partition_point(|&(_, b)| b <= start);
        let last = self.committed.partition_point(|&(a, _)| a < end);
        if first >= last { return; }
        let lo = self.committed[first].0;
        let hi = self.committed[last - 1].1;
        let remnants = [(lo, start), (end, hi)].into_iter().filter(|&(a,b)| a < b);
        self.committed.splice(first..last, remnants);
    }
}

#[derive(Default)]
struct Ledger {
    reservations: BTreeMap<usize, Reservation>,
    /// Successful operations outside known reservations (e.g. reservations created
    /// by a path not instrumented here) are intentionally not guessed at.
    untracked_operations: u64,
}
impl Ledger {
    fn alloc(&mut self, requested: usize, returned: usize, size: usize, flags: u32) {
        if returned == 0 || size == 0 { return; }
        let start = returned;
        let origin = if requested == 0 { returned } else { requested };
        let Some(end) = origin.checked_add(size).and_then(|n| n.checked_next_multiple_of(PAGE)) else {
            self.untracked_operations = self.untracked_operations.saturating_add(1);
            return;
        };
        if flags & mem::MEM_RESERVE != 0 || (requested == 0 && flags & mem::MEM_COMMIT != 0) {
            let mut r = Reservation { end, committed: Vec::new() };
            if flags & mem::MEM_COMMIT != 0 { r.commit(start, end); }
            self.reservations.insert(start, r);
        } else if flags & mem::MEM_COMMIT != 0 {
            if let Some((_, r)) = self.reservations.range_mut(..=start).next_back().filter(|(_, r)| end <= r.end) {
                r.commit(start, end);
            } else {
                self.untracked_operations = self.untracked_operations.saturating_add(1);
            }
        }
    }

    fn free(&mut self, at: usize, size: usize, flags: u32) {
        if flags == mem::MEM_RELEASE {
            if self.reservations.remove(&at).is_none() {
                self.untracked_operations = self.untracked_operations.saturating_add(1);
            }
        } else if flags == mem::MEM_DECOMMIT {
            let start = at & !(PAGE - 1);
            let Some((&base, r)) = self.reservations.range_mut(..=start).next_back().filter(|(_, r)| start < r.end) else {
                self.untracked_operations = self.untracked_operations.saturating_add(1);
                return;
            };
            let end = if size == 0 && start == base { Some(r.end) } else if size != 0 {
                at.checked_add(size).and_then(|n| n.checked_next_multiple_of(PAGE))
            } else { None };
            if let Some(end) = end.filter(|&end| end <= r.end) {
                r.decommit(start, end);
            } else {
                self.untracked_operations = self.untracked_operations.saturating_add(1);
            }
        }
    }

    fn snapshot(&self) -> (usize, u64, u64) {
        let bytes = self.reservations.values().flat_map(|r| r.committed.iter())
            .fold(0u64, |sum, &(a,b)| sum.saturating_add((b - a) as u64));
        (self.reservations.len(), bytes, self.untracked_operations)
    }
}

static LIVE: Mutex<Option<Ledger>> = Mutex::new(None);

// The ledger lock is held only around the raw Windows call, not the other kernelx
// allocators, hooks or GPU paths. The caller may hold AWAY; we never take it here.
pub(super) unsafe fn alloc(addr: *const c_void, size: usize, flags: u32, protect: u32) -> *mut c_void {
    let mut g = LIVE.lock().unwrap_or_else(|e| e.into_inner());
    let q = mem::VirtualAlloc(addr, size, flags, protect);
    let error = GetLastError();
    if !q.is_null() {
        g.get_or_insert_with(Ledger::default).alloc(addr as usize, q as usize, size, flags);
    }
    SetLastError(error);
    q
}

pub(super) unsafe fn free(addr: *mut c_void, size: usize, flags: u32) -> i32 {
    let mut g = LIVE.lock().unwrap_or_else(|e| e.into_inner());
    let ok = mem::VirtualFree(addr, size, flags);
    let error = GetLastError();
    if ok != 0 { g.get_or_insert_with(Ledger::default).free(addr as usize, size, flags); }
    SetLastError(error);
    ok
}

pub(super) fn snapshot() -> Option<(usize, u64, u64)> {
    LIVE.try_lock().ok().map(|g| g.as_ref().map_or((0,0,0), Ledger::snapshot))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recommits_partial_decommit_and_address_reuse_are_not_double_counted() {
        let mut l = Ledger::default();
        let base = 0x10000;
        l.alloc(0, base, 8 * PAGE, mem::MEM_RESERVE);
        assert_eq!(l.snapshot(), (1, 0, 0));
        l.alloc(base, base, 4 * PAGE, mem::MEM_COMMIT);
        l.alloc(base + PAGE, base + PAGE, 5 * PAGE, mem::MEM_COMMIT);
        assert_eq!(l.snapshot().1, 6 * PAGE as u64);
        l.free(base + 2 * PAGE + 1, PAGE, mem::MEM_DECOMMIT);
        assert_eq!(l.snapshot().1, 4 * PAGE as u64);
        l.alloc(base + 2 * PAGE, base + 2 * PAGE, 2 * PAGE, mem::MEM_COMMIT);
        assert_eq!(l.snapshot().1, 6 * PAGE as u64);
        l.free(base, 0, mem::MEM_RELEASE);
        assert_eq!(l.snapshot(), (0, 0, 0));
        l.alloc(0, base, PAGE, mem::MEM_RESERVE | mem::MEM_COMMIT);
        assert_eq!(l.snapshot(), (1, PAGE as u64, 0));
        l.free(base, 0, mem::MEM_DECOMMIT);
        assert_eq!(l.snapshot(), (1, 0, 0));
        l.alloc(0, 0, PAGE, mem::MEM_RESERVE | mem::MEM_COMMIT);
        assert_eq!(l.snapshot(), (1, 0, 0));
    }
}
