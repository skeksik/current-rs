//! The swap chain handed to the UI (ADR 0007, ADR 0019 "Swap chain"): mpv's `display-swapchain` is a borrowed `IDXGISwapChain*`, so
//! the engine takes its own reference through `IUnknown`'s vtable (no Windows crate needed) and the receiver gives it back with
//! [`SwapChain::release`] or by dropping it.

// `IUnknown::AddRef` and `Release` are calls through a C vtable, which needs `unsafe`; this module is one of the two places allowed
// (crate manifest).
#![allow(unsafe_code)]

use std::ffi::c_void;

/// The first three entries of every COM interface's vtable (`IUnknown`, `unknwn.h`).
#[repr(C)]
struct IUnknownVtbl {
    query_interface: unsafe extern "system" fn(*mut c_void, *const c_void, *mut *mut c_void) -> i32,
    add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
    release: unsafe extern "system" fn(*mut c_void) -> u32,
}

/// An owned reference to mpv's swap chain. `handle` is the `IDXGISwapChain*` as an integer, for `SetSwapChain`; `generation` grows by
/// one with every new swap chain, so the UI can tell a recreated VO from a repeat.
#[derive(Debug)]
pub struct SwapChain {
    // The address, 0 once released. An integer, so the type is `Send` without an `unsafe impl`: COM reference counting on DXGI objects
    // is thread-safe (not verified beyond DXGI being free-threaded).
    ptr: usize,
    generation: u64,
}

impl SwapChain {
    /// Takes a reference to the COM object at `ptr`.
    ///
    /// # Safety
    /// `ptr` must point to a live COM object (its first field a pointer to an `IUnknown`-compatible vtable) for the duration of this
    /// call. For `display-swapchain` that holds while mpv's VO keeps the swap chain, which it does until the VO is recreated or the
    /// player ends; the event thread calls this as soon as the property change arrives.
    pub(crate) unsafe fn add_ref(ptr: *mut c_void, generation: u64) -> SwapChain {
        // SAFETY: per the contract above, `ptr` is a live COM object whose first field is its vtable pointer.
        unsafe {
            let vtbl = *(ptr as *const *const IUnknownVtbl);
            ((*vtbl).add_ref)(ptr);
        }
        SwapChain {
            ptr: ptr as usize,
            generation,
        }
    }

    pub fn handle(&self) -> u64 {
        self.ptr as u64
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Gives the reference back. Call it after `SetSwapChain` took its own.
    pub fn release(self) {
        drop(self);
    }
}

impl Drop for SwapChain {
    fn drop(&mut self) {
        if self.ptr == 0 {
            return;
        }
        let ptr = self.ptr as *mut c_void;
        self.ptr = 0;
        // SAFETY: `add_ref` took a reference on this object, so it is alive until this matching `Release`; the pointer was zeroed
        // first, so it is released exactly once.
        unsafe {
            let vtbl = *(ptr as *const *const IUnknownVtbl);
            ((*vtbl).release)(ptr);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicI32, Ordering};

    // A minimal COM object: a vtable pointer followed by a reference count.
    #[repr(C)]
    struct Fake {
        vtbl: *const IUnknownVtbl,
        refs: AtomicI32,
    }

    unsafe extern "system" fn qi(_: *mut c_void, _: *const c_void, _: *mut *mut c_void) -> i32 {
        -1
    }
    unsafe extern "system" fn add_ref(this: *mut c_void) -> u32 {
        // SAFETY: only called with a `Fake` below.
        let fake = unsafe { &*(this as *const Fake) };
        (fake.refs.fetch_add(1, Ordering::SeqCst) + 1) as u32
    }
    unsafe extern "system" fn release(this: *mut c_void) -> u32 {
        // SAFETY: only called with a `Fake` below.
        let fake = unsafe { &*(this as *const Fake) };
        (fake.refs.fetch_sub(1, Ordering::SeqCst) - 1) as u32
    }
    static VTBL: IUnknownVtbl = IUnknownVtbl {
        query_interface: qi,
        add_ref,
        release,
    };

    #[test]
    fn add_ref_then_release_once() {
        let fake = Fake {
            vtbl: &VTBL,
            refs: AtomicI32::new(1),
        };
        let ptr = &fake as *const Fake as *mut c_void;
        // SAFETY: `fake` outlives both swap chains.
        let a = unsafe { SwapChain::add_ref(ptr, 1) };
        let b = unsafe { SwapChain::add_ref(ptr, 2) };
        assert_eq!(fake.refs.load(Ordering::SeqCst), 3);
        assert_eq!(a.handle(), ptr as u64);
        assert_eq!(b.generation(), 2);
        a.release();
        assert_eq!(fake.refs.load(Ordering::SeqCst), 2);
        drop(b);
        assert_eq!(fake.refs.load(Ordering::SeqCst), 1);
    }
}
