//! Self-modifying code detection via signal handler (Phase 33).
//!
//! On Linux, executable pages are mapped RX (no write) at the host level. A
//! fastmem write to such a page triggers SIGSEGV. The signal handler catches
//! it, temporarily upgrades the page to RWX, and records the event for the
//! dispatch loop to process.
//!
//! On non-Linux platforms, all functions are no-ops and SMC detection relies
//! entirely on the slowmem software check path.

#![allow(dead_code)]

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

const MAX_EXEC_PAGES: usize = 1024;
const MAX_SMC_EVENTS: usize = 256;

static EXEC_PAGE_BASES: [AtomicU64; MAX_EXEC_PAGES] = {
    // SAFETY: AtomicU64 is valid for all bit patterns including zero.
    [const { AtomicU64::new(0) }; MAX_EXEC_PAGES]
};
static EXEC_PAGE_COUNT: AtomicUsize = AtomicUsize::new(0);

static SMC_EVENTS: [AtomicU64; MAX_SMC_EVENTS] = {
    // SAFETY: AtomicU64 is valid for all bit patterns including zero.
    [const { AtomicU64::new(0) }; MAX_SMC_EVENTS]
};
static SMC_EVENT_HEAD: AtomicUsize = AtomicUsize::new(0);
static SMC_EVENT_TAIL: AtomicUsize = AtomicUsize::new(0);

/// Host page addresses that were upgraded to RWX by the signal handler and
/// need to be re-protected back to RX after the dispatch loop processes the
/// SMC events.
static REPROTECT_EVENTS: [AtomicU64; MAX_SMC_EVENTS] = {
    // SAFETY: AtomicU64 is valid for all bit patterns including zero.
    [const { AtomicU64::new(0) }; MAX_SMC_EVENTS]
};
static REPROTECT_HEAD: AtomicUsize = AtomicUsize::new(0);
static REPROTECT_TAIL: AtomicUsize = AtomicUsize::new(0);

// ── Public API (cross-platform, real impl gated) ──────────────────────────

/// Register a page base as containing executable code. Called from
/// `GuestMemory::map_page` when the page has `execute: true`.
pub fn register_executable_page(page_base: u64) {
    platform::register_executable_page_impl(page_base);
}

/// Unregister a page base. Called from `GuestMemory::unmap_page`.
pub fn unregister_executable_page(page_base: u64) {
    platform::unregister_executable_page_impl(page_base);
}

/// Drain all pending SMC events from the signal handler ring buffer. Returns
/// the list of written addresses that triggered SMC invalidation. Called from
/// the dispatch loop between block executions.
#[must_use]
pub fn drain_smc_events() -> Vec<u64> {
    platform::drain_smc_events_impl()
}

/// Install the SIGSEGV handler for SMC detection. `arena_base` and
/// `arena_size` define the guest memory region so the handler can reject
/// faults outside the arena. No-op on non-Linux platforms.
pub fn install_smc_handler(arena_base: usize, arena_size: u64) -> Result<(), crate::VmmFault> {
    platform::install_smc_handler_impl(arena_base, arena_size)
}

/// Uninstall the SMC signal handler. No-op on non-Linux platforms.
pub fn uninstall_smc_handler() {
    platform::uninstall_smc_handler_impl();
}

/// Drain host page addresses that were upgraded to RWX by the signal handler
/// and re-protect them back to RX. Called from the dispatch loop after
/// processing SMC events. No-op on non-Linux platforms.
pub fn reprotect_pages() {
    platform::reprotect_pages_impl();
}

// ── Internal helpers (cross-platform) ─────────────────────────────────────

fn push_smc_event(write_address: u64) {
    let tail = SMC_EVENT_TAIL.load(Ordering::Relaxed);
    let next = (tail + 1) % MAX_SMC_EVENTS;
    // Drop events if the ring is full — the dispatch loop drains faster than
    // the signal handler can fill it in practice.
    if next == SMC_EVENT_HEAD.load(Ordering::Acquire) {
        return;
    }
    SMC_EVENTS[tail].store(write_address, Ordering::Release);
    SMC_EVENT_TAIL.store(next, Ordering::Release);
}

fn push_reprotect_event(host_page_addr: u64) {
    let tail = REPROTECT_TAIL.load(Ordering::Relaxed);
    let next = (tail + 1) % MAX_SMC_EVENTS;
    if next == REPROTECT_HEAD.load(Ordering::Acquire) {
        return;
    }
    REPROTECT_EVENTS[tail].store(host_page_addr, Ordering::Release);
    REPROTECT_TAIL.store(next, Ordering::Release);
}

fn is_executable_page(page_base: u64) -> bool {
    let count = EXEC_PAGE_COUNT.load(Ordering::Acquire);
    // Linear scan — pages are few and the signal handler is rare.
    (0..count).any(|i| EXEC_PAGE_BASES[i].load(Ordering::Acquire) == page_base)
}

// ── Linux implementation ──────────────────────────────────────────────────

#[cfg(target_os = "linux")]
#[allow(unsafe_code)]
mod platform {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::{
        EXEC_PAGE_BASES, EXEC_PAGE_COUNT, MAX_EXEC_PAGES, MAX_SMC_EVENTS, REPROTECT_EVENTS,
        REPROTECT_HEAD, REPROTECT_TAIL, SMC_EVENT_HEAD, SMC_EVENT_TAIL, SMC_EVENTS,
        is_executable_page, push_reprotect_event, push_smc_event,
    };

    static mut ARENA_BASE: usize = 0;
    static mut ARENA_SIZE: u64 = 0;
    static HANDLER_INSTALLED: AtomicUsize = AtomicUsize::new(0);

    pub(super) fn register_executable_page_impl(page_base: u64) {
        let count = EXEC_PAGE_COUNT.load(Ordering::Acquire);
        if count >= MAX_EXEC_PAGES {
            return;
        }
        // Check for duplicates before inserting.
        for i in 0..count {
            if EXEC_PAGE_BASES[i].load(Ordering::Acquire) == page_base {
                return;
            }
        }
        EXEC_PAGE_BASES[count].store(page_base, Ordering::Release);
        EXEC_PAGE_COUNT.store(count + 1, Ordering::Release);
    }

    pub(super) fn unregister_executable_page_impl(page_base: u64) {
        let count = EXEC_PAGE_COUNT.load(Ordering::Acquire);
        for i in 0..count {
            if EXEC_PAGE_BASES[i].load(Ordering::Acquire) == page_base {
                // Swap with last and shrink.
                if i < count - 1 {
                    let last = EXEC_PAGE_BASES[count - 1].load(Ordering::Acquire);
                    EXEC_PAGE_BASES[i].store(last, Ordering::Release);
                }
                EXEC_PAGE_COUNT.store(count - 1, Ordering::Release);
                return;
            }
        }
    }

    pub(super) fn drain_smc_events_impl() -> Vec<u64> {
        let mut events = Vec::new();
        loop {
            let head = SMC_EVENT_HEAD.load(Ordering::Acquire);
            let tail = SMC_EVENT_TAIL.load(Ordering::Acquire);
            if head == tail {
                break;
            }
            let value = SMC_EVENTS[head].load(Ordering::Acquire);
            let next = (head + 1) % MAX_SMC_EVENTS;
            SMC_EVENT_HEAD.store(next, Ordering::Release);
            events.push(value);
        }
        events
    }

    pub(super) fn drain_reprotect_events_impl() -> Vec<u64> {
        let mut events = Vec::new();
        loop {
            let head = REPROTECT_HEAD.load(Ordering::Acquire);
            let tail = REPROTECT_TAIL.load(Ordering::Acquire);
            if head == tail {
                break;
            }
            let value = REPROTECT_EVENTS[head].load(Ordering::Acquire);
            let next = (head + 1) % MAX_SMC_EVENTS;
            REPROTECT_HEAD.store(next, Ordering::Release);
            events.push(value);
        }
        events
    }

    pub(super) fn reprotect_pages_impl() {
        for host_page in drain_reprotect_events_impl() {
            // SAFETY: these pages were originally mapped by the arena and
            // temporarily upgraded to RWX by the signal handler. Restoring
            // RX is safe and matches the original page permissions.
            unsafe {
                libc::mprotect(
                    host_page as *mut libc::c_void,
                    4096,
                    libc::PROT_READ | libc::PROT_EXEC,
                );
            }
        }
    }

    pub(super) fn install_smc_handler_impl(
        arena_base: usize,
        arena_size: u64,
    ) -> Result<(), crate::VmmFault> {
        use crate::VmmFault;
        use std::sync::atomic::AtomicUsize;

        if HANDLER_INSTALLED
            .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Ok(());
        }

        // SAFETY: writing to static muts during single-threaded init before
        // the handler is installed. The `HANDLER_INSTALLED` flag gates access.
        unsafe {
            ARENA_BASE = arena_base;
            ARENA_SIZE = arena_size;
        }

        // SAFETY: `sigaction` is safe to call with a valid `sigaction` struct.
        // The handler is async-signal-safe: it only does atomic ops and
        // `mprotect` (both are AS-safe by POSIX).
        unsafe {
            let mut sa: libc::sigaction = std::mem::zeroed();
            sa.sa_sigaction = smc_sigsegv_handler as usize;
            sa.sa_flags = libc::SA_SIGINFO | libc::SA_RESTART;
            libc::sigemptyset(&mut sa.sa_mask);
            if libc::sigaction(libc::SIGSEGV, &sa, std::ptr::null_mut()) != 0 {
                HANDLER_INSTALLED.store(0, Ordering::Release);
                return Err(VmmFault::ArenaReservation {
                    message: "failed to install SIGSEGV handler".to_owned(),
                });
            }
        }
        Ok(())
    }

    pub(super) fn uninstall_smc_handler_impl() {
        if HANDLER_INSTALLED
            .compare_exchange(1, 0, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return;
        }
        // SAFETY: restoring the default SIGSEGV handler. After this, faults
        // outside the arena or on non-executable pages will crash normally.
        unsafe {
            let mut sa: libc::sigaction = std::mem::zeroed();
            sa.sa_sigaction = libc::SIG_DFL;
            libc::sigemptyset(&mut sa.sa_mask);
            libc::sigaction(libc::SIGSEGV, &sa, std::ptr::null_mut());
        }
    }

    /// Async-signal-safe SIGSEGV handler. Only touches atomics and calls
    /// `mprotect`, both of which are async-signal-safe by POSIX.
    extern "C" fn smc_sigsegv_handler(
        _sig: libc::c_int,
        info: *mut libc::siginfo_t,
        _ucontext: *mut libc::c_void,
    ) {
        // SAFETY: the kernel guarantees `info` is valid for SA_SIGINFO handlers.
        let fault_addr = unsafe { (*info).si_addr } as usize;

        // SAFETY: reading static muts that are set once during init.
        let arena_base = unsafe { ARENA_BASE };
        let arena_size = unsafe { ARENA_SIZE };

        if arena_base == 0 {
            // No arena — not our fault.
            restore_default_and_reraise(fault_addr);
            return;
        }

        let end = arena_base.wrapping_add(arena_size as usize);
        if fault_addr < arena_base || fault_addr >= end {
            // Outside guest arena — not our fault.
            restore_default_and_reraise(fault_addr);
            return;
        }

        // Convert host address to guest address. The arena is mapped at
        // `arena_base`, so guest address = fault_addr - arena_base.
        let guest_addr = fault_addr.wrapping_sub(arena_base);
        let page_base = (guest_addr & !0xFFF) as u64;
        if !is_executable_page(page_base) {
            // Page is not tracked as executable — not our fault.
            restore_default_and_reraise(fault_addr);
            return;
        }

        // Temporarily upgrade the page to RWX so the faulting instruction
        // can retry and succeed. Record the host page address for later
        // re-protection back to RX.
        let host_page = fault_addr & !0xFFF;
        let page_ptr = host_page as *mut libc::c_void;
        // SAFETY: the address is page-aligned and inside the arena reservation.
        unsafe {
            libc::mprotect(
                page_ptr,
                4096,
                libc::PROT_READ | libc::PROT_WRITE | libc::PROT_EXEC,
            );
        }

        // Record the guest address for the dispatch loop.
        push_smc_event(guest_addr as u64);
        // Record the host page for re-protection after dispatch processes the event.
        push_reprotect_event(host_page as u64);
    }

    fn restore_default_and_reraise(fault_addr: usize) {
        // SAFETY: restoring default handler and re-raising. If we got here,
        // the fault is genuinely invalid and should crash.
        unsafe {
            let mut sa: libc::sigaction = std::mem::zeroed();
            sa.sa_sigaction = libc::SIG_DFL;
            libc::sigemptyset(&mut sa.sa_mask);
            libc::sigaction(libc::SIGSEGV, &sa, std::ptr::null_mut());
            libc::raise(libc::SIGSEGV);
        }
        let _ = fault_addr; // used only for clarity
    }
}

// ── Non-Linux stubs ───────────────────────────────────────────────────────

#[cfg(not(target_os = "linux"))]
mod platform {
    use std::sync::atomic::Ordering;

    use super::{
        MAX_SMC_EVENTS, REPROTECT_EVENTS, REPROTECT_HEAD, REPROTECT_TAIL, SMC_EVENT_HEAD,
        SMC_EVENT_TAIL, SMC_EVENTS,
    };

    pub(super) fn register_executable_page_impl(_page_base: u64) {}
    pub(super) fn unregister_executable_page_impl(_page_base: u64) {}

    pub(super) fn drain_smc_events_impl() -> Vec<u64> {
        let mut events = Vec::new();
        loop {
            let head = SMC_EVENT_HEAD.load(Ordering::Acquire);
            let tail = SMC_EVENT_TAIL.load(Ordering::Acquire);
            if head == tail {
                break;
            }
            let value = SMC_EVENTS[head].load(Ordering::Acquire);
            let next = (head + 1) % MAX_SMC_EVENTS;
            SMC_EVENT_HEAD.store(next, Ordering::Release);
            events.push(value);
        }
        events
    }

    pub(super) fn install_smc_handler_impl(
        _arena_base: usize,
        _arena_size: u64,
    ) -> Result<(), crate::VmmFault> {
        Ok(())
    }

    pub(super) fn drain_reprotect_events_impl() -> Vec<u64> {
        let mut events = Vec::new();
        loop {
            let head = REPROTECT_HEAD.load(Ordering::Acquire);
            let tail = REPROTECT_TAIL.load(Ordering::Acquire);
            if head == tail {
                break;
            }
            let value = REPROTECT_EVENTS[head].load(Ordering::Acquire);
            let next = (head + 1) % MAX_SMC_EVENTS;
            REPROTECT_HEAD.store(next, Ordering::Release);
            events.push(value);
        }
        events
    }

    pub(super) fn reprotect_pages_impl() {}

    pub(super) fn uninstall_smc_handler_impl() {}
}
