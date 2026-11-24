//! The host process: started by the launcher with the game's command line. Sets up
//! logging, the VFS, hooks and HLE, maps the title and jumps to its entry point.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::config::Boot;

pub static HOST_MODE: AtomicBool = AtomicBool::new(false);

pub fn host_main(boot_path: &Path) -> i32 {
    let boot: Boot = match std::fs::read_to_string(boot_path).map_err(|e| e.to_string()).and_then(|s| serde_json::from_str(&s).map_err(|e| e.to_string())) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("xcl host: cannot read {}: {e}", boot_path.display());
            return 0x7C1_0001;
        }
    };
    crate::log::init(
        Some(&boot.logs_dir().join("xcl.log")),
        crate::log::Level::parse(&boot.config.log_level),
        boot.config.log_to_stderr,
    );
    let detailed = if std::env::var("XCL_DETAILED").is_ok_and(|v| v.trim() == "0") {
        false
    } else {
        match crate::log::init_detail(&boot.logs_dir().join("detailed.txt")) {
            Ok(()) => true,
            Err(e) => { crate::xlog_warn!("Cannot open detailed.txt: {e}"); false }
        }
    };
    crate::log::detail(format_args!("host_start xcl_version={} pid={}", env!("CARGO_PKG_VERSION"), std::process::id()));
    crate::xlog_info!("XCL {} – host for '{}'", env!("CARGO_PKG_VERSION"), boot.title);
    crate::xlog_info!("game: {} (read-only folder: {})", boot.exe_path.display(), boot.game_root.display());
    crate::xlog_info!("XCL data: {}", boot.data_dir.display());

    if std::env::var_os("XCL_UMA_MB").is_none() && boot.config.uma_mb != 0 {
        std::env::set_var("XCL_UMA_MB", boot.config.uma_mb.to_string());
    }
    crate::state::set_boot(boot.clone());
    // `XCL_ACTIVITY=1`: every 2 s what the title did since (a timeline of where it stops)
    let activity = std::env::var_os("XCL_ACTIVITY").is_some();
    if activity || detailed {
        if let Err(e) = std::thread::Builder::new().name("xcl-activity".into()).spawn(move || {
            let snap = || {
                let (p, e) = crate::hle::d3d12x::present::activity();
                let (d, x) = crate::hle::d3d12x::bind::activity();
                let f = crate::hle::d3d12x::frame::activity();
                let (r, b, fail) = crate::hle::dstorage::counters();
                [p, e, d, x, f[0], f[1], f[2], f[3], r, b >> 20, fail]
            };
            let mut last = snap();
            loop {
                std::thread::sleep(std::time::Duration::from_secs(2));
                let now = snap();
                let d = activity_delta(now, last);
                last = now;
                let (shown, boundaries) = crate::hle::d3d12x::present::counters();
                let failures = crate::hle::d3d12x::present::failure_count();
                crate::log::detail(format_args!("presentation shown_total={shown} presentx_total={boundaries} failed_attempts_total={failures}"));
                let (built, failed, tracked) = crate::hle::d3d12x::pso_async::diagnostic_counts();
                let (tex, uploads, skipped) = crate::hle::d3d12x::title_textures::diagnostic_counts();
                let (hits, misses, compiled, compile_failed) = crate::gpu::recomp::dxc::diagnostic_counts();
                crate::log::detail(format_args!(
                    "activity delta_presents={} delta_submits={} delta_draws={} delta_dispatches={} delta_frame_waits={} delta_frame_timeouts={} delta_frame_stats={} delta_not_flipped={} delta_storage_requests={} delta_storage_MiB={} delta_storage_failed={} pso_built_total={built} pso_failed_total={failed} pso_tracked={tracked:?} textures={tex} texture_uploads_total={uploads} texture_unchanged_total={skipped} dxil_hits={hits} dxil_misses={misses} dxil_compiles={compiled} dxil_failures={compile_failed}",
                    d[0], d[1], d[2], d[3], d[4], d[5], d[6], d[7], d[8], d[9], d[10]
                ));
                if activity { crate::xlog_info!(
                    "activity (2 s): {} presents, {} submits, {} draws, {} dispatches, {} frame-event waits ({} timed out), {} frame statistics ({} not flipped); DirectStorage {} requests, {} MB, {} failed",
                    d[0], d[1], d[2], d[3], d[4], d[5], d[6], d[7], d[8], d[9], d[10]
                ); }
                // where the memory is: the process's commit, video memory, and XCL's parts of it
                let private = unsafe {
                    use windows_sys::Win32::System::ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX};
                    let mut c: PROCESS_MEMORY_COUNTERS_EX = std::mem::zeroed();
                    c.cb = std::mem::size_of_val(&c) as u32;
                    let ok = K32GetProcessMemoryInfo(windows_sys::Win32::System::Threading::GetCurrentProcess(), &mut c as *mut _ as *mut _, c.cb);
                    (ok != 0).then_some(c.PrivateUsage as u64)
                };
                let system = unsafe {
                    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
                    let mut m: MEMORYSTATUSEX = std::mem::zeroed();
                    m.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
                    (GlobalMemoryStatusEx(&mut m) != 0).then_some((m.ullAvailPhys >> 20, m.ullAvailPageFile >> 20, m.dwMemoryLoad))
                };
                crate::log::detail(format_args!("system_memory (available_phys_MiB, available_commit_MiB, load_percent)={system:?} private_MiB={:?}", private.map(|v| v >> 20)));
                let (xmem, physical, plain) = crate::hle::kernelx::title_allocation_diagnostics();
                crate::log::detail(format_args!("title_allocations xmem_default_live=(count,logical_bytes,incomplete):{xmem:?} physical_live_logical_bytes={physical:?} plain_virtual=(reservations,explicit_commit_bytes,untracked_ops):{plain:?} coverage=partial_not_total_commit"));
                let video = crate::hle::d3d12x::real::video_memory();
                let (arenas, arenas_made) = crate::hle::d3d12x::vram::arena_bytes();
                let (pools, pools_mapped) = crate::hle::d3d12x::sparse::pool_bytes();
                let (pages, page_pools) = crate::hle::d3d12x::vram::page_bytes();
                let mb = |b: u64| b >> 20;
                let private = private.map(mb);
                let local = video.map(|v| mb(v.0));
                let non_local = video.map(|v| mb(v.1));
                let memory = format!(
                    "memory private_MiB={private:?} local_MiB={local:?} non_local_MiB={non_local:?} texture_pages_MiB={} texture_pools_MiB={} arenas_used_MiB={} arenas_made_MiB={} sparse_pools_MiB={} sparse_mapped_MiB={}",
                    mb(pages as u64), mb(page_pools as u64), mb(arenas as u64), mb(arenas_made as u64), mb(pools as u64), mb(pools_mapped as u64)
                );
                crate::log::detail(format_args!("{memory}"));
                let (heaps, free, cooling, idle, targets, candidates, blocked) = crate::hle::d3d12x::vram::pool_diagnostics();
                crate::log::detail(format_args!("texture_pool heap_count={heaps} free_tiles={free} cooling_tiles={cooling} idle_queue_entries={idle} tracked_targets={targets} fully_recyclable_pool_candidates={candidates} mapping_fence_blocked_tiles={blocked} tile_bytes=65536"));
                if activity {
                    // Keep the legacy smoke.ps1 format for a successful measurement.
                    if let Some(private) = private {
                        crate::xlog_info!("memory: {private} MB committed by the process; {memory}");
                    } else {
                        crate::xlog_info!("{memory}");
                    }
                }
            }
        }) {
            crate::log::detail(format_args!("activity_worker_start_failed"));
            crate::xlog_warn!("Could not start activity worker: {e}");
        }
    }
    // `XCL_THREAD_DUMP_AT=s1,s2…`: where every thread is at those seconds (a stalled title)
    if let Ok(v) = std::env::var("XCL_THREAD_DUMP_AT") {
        let at: Vec<f64> = v.split(',').filter_map(|s| s.trim().parse::<f64>().ok()).filter(|s| valid_dump_time(*s)).collect();
        let _ = std::thread::Builder::new().name("xcl-thread-dump".into()).spawn(move || {
            let t0 = std::time::Instant::now();
            for s in at {
                let left = s - t0.elapsed().as_secs_f64();
                if left > 0.0 {
                    std::thread::sleep(std::time::Duration::from_secs_f64(left));
                }
                crate::eh::dump_threads();
            }
        });
    }
    // `XCL_RUN_SECONDS=n`: the title ends itself after n seconds – the GPU drained, the log and
    // report written (a process killed from outside with GPU work in flight can hang in the driver)
    if let Some(secs) = std::env::var("XCL_RUN_SECONDS").ok().and_then(|v| v.parse::<u64>().ok()) {
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(secs));
            let (shown, presented) = crate::hle::d3d12x::present::counters();
            crate::xlog_info!("XCL_RUN_SECONDS: {secs} s – ending the title ({shown} frames shown, {presented} PresentX calls)");
            crate::report::write_final(None);
            crate::log::flush();
            crate::hle::d3d12x::real::end_title(0);
        });
    }
    crate::stubs::init_db(&boot.config.stub_returns);
    crate::hle::init(&boot.config);
    crate::hle::kernelx::fine_timer();
    crate::loader::set_cache_dir(boot.cache_dir());
    let mut dirs = vec![boot.game_root.clone()];
    if let Some(p) = boot.exe_path.parent() {
        if p != boot.game_root {
            dirs.insert(0, p.to_path_buf());
        }
    }
    crate::resolver::set_search_dirs(dirs);

    let mounts: Vec<(String, String)> = boot.config.mounts.iter().map(|(k, v)| (k.clone(), boot.expand(v))).collect();
    // (the shader cache is XCL's too: on a drive a console mount names it must stay where it is)
    crate::vfs::init(crate::vfs::Vfs::new(&boot.game_root, &boot.data_dir, mounts).own_folder(&crate::gpu::recomp::dxc::cache_dir()));
    let _ = std::fs::create_dir_all(boot.data_dir.join("temp"));

    if let Err(e) = crate::vfs_hooks::install() {
        crate::xlog_error!("VFS: not all hooks installed: {e}");
    }
    if let Err(e) = crate::eh::install_pc_hook() {
        crate::xlog_error!("hook RtlPcToFileHeader: {e} – C++ exceptions in the game may not work");
    }
    crate::eh::install_crash_handler();
    crate::eh::start_probes();
    HOST_MODE.store(true, Ordering::Release);

    let main = match crate::loader::load_main(&boot.exe_path) {
        Ok(m) => m,
        Err(e) => {
            crate::xlog_error!("loading the game failed: {e}");
            crate::report::write_final(Some(format!("loader: {e}")));
            return 0x7C1_0002;
        }
    };
    unsafe { crate::hle::kernelx::adopt_title_hooks(&main) };
    crate::log::detail(format_args!("loader_complete"));
    crate::report::write_snapshot();
    let code = crate::loader::run_main(&main);
    crate::xlog_info!("game entry point returned code {code}");
    crate::report::write_final(None);
    crate::log::flush();
    code as i32
}

fn activity_delta(now: [u64; 11], last: [u64; 11]) -> [u64; 11] {
    std::array::from_fn(|i| now[i].saturating_sub(last[i]))
}

fn valid_dump_time(s: f64) -> bool {
    // Duration::from_secs_f64 panics on NaN/infinity or unrepresentable values.
    s.is_finite() && s >= 0.0 && s < (u64::MAX as f64) / 2.0
}

#[cfg(test)]
mod diagnostic_tests {
    use super::*;
    #[test]
    fn reset_counters_do_not_underflow() {
        assert_eq!(activity_delta([2; 11], [5; 11]), [0; 11]);
        assert_eq!(activity_delta([5; 11], [2; 11]), [3; 11]);
    }
    #[test]
    fn rejects_invalid_thread_dump_times() {
        assert!(valid_dump_time(0.0));
        assert!(valid_dump_time(2.5));
        for s in [f64::NAN, f64::INFINITY, -1.0, f64::MAX] { assert!(!valid_dump_time(s)); }
    }
}
