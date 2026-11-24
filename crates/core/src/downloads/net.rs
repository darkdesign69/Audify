//! Is there a network, and is it metered? macOS and iOS answer both through Network.framework's
//! `NWPathMonitor` (status, `isExpensive` = cellular / iPhone hotspot, `isConstrained` = Low Data
//! Mode). It pushes changes, so going offline greys the library at once and coming back resumes
//! the queue without polling. Elsewhere (and when the monitor can't start) we assume online and
//! let failed fetches, which are probed with `http::network_reachable`, say otherwise.

/// `cb(online, metered)` on every path change, from a dispatch queue thread.
pub fn start(cb: impl Fn(bool, bool) + Send + Sync + 'static) -> bool {
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    {
        mac::start(cb);
        true
    }
    #[cfg(not(any(target_os = "macos", target_os = "ios")))]
    {
        let _ = cb;
        false
    }
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
mod mac {
    use std::ffi::c_void;

    use block2::RcBlock;

    #[link(name = "Network", kind = "framework")]
    extern "C" {
        fn nw_path_monitor_create() -> *mut c_void;
        fn nw_path_monitor_set_queue(monitor: *mut c_void, queue: *mut c_void);
        fn nw_path_monitor_set_update_handler(
            monitor: *mut c_void,
            handler: &block2::Block<dyn Fn(*mut c_void)>,
        );
        fn nw_path_monitor_start(monitor: *mut c_void);
        fn nw_path_get_status(path: *mut c_void) -> i32;
        fn nw_path_is_expensive(path: *mut c_void) -> bool;
        fn nw_path_is_constrained(path: *mut c_void) -> bool;
    }
    extern "C" {
        fn dispatch_get_global_queue(identifier: isize, flags: usize) -> *mut c_void;
    }

    /// `nw_path_status_satisfied`.
    const SATISFIED: i32 = 1;

    pub fn start(cb: impl Fn(bool, bool) + Send + Sync + 'static) {
        // SAFETY: plain C calls into Network.framework. The monitor and the handler block are
        // leaked on purpose: they live as long as the app, and the monitor copies the block.
        unsafe {
            let monitor = nw_path_monitor_create();
            if monitor.is_null() {
                return;
            }
            let block = RcBlock::new(move |path: *mut c_void| {
                if path.is_null() {
                    return;
                }
                let online = nw_path_get_status(path) == SATISFIED;
                let metered = nw_path_is_expensive(path) || nw_path_is_constrained(path);
                cb(online, metered);
            });
            nw_path_monitor_set_update_handler(monitor, &block);
            nw_path_monitor_set_queue(monitor, dispatch_get_global_queue(0, 0));
            nw_path_monitor_start(monitor);
            std::mem::forget(block);
        }
    }
}
