//! Plain C ABI for the iOS Swift module. Mirrors `jni_bridge`'s surface
//! exactly (same `Instance` methods, same pixel format out) but as
//! ordinary `extern "C"` functions instead of JNI exports, since Swift
//! calls into Rust through a bridging header rather than through a VM.
//!
//! **Not built or tested on this machine** — this crate's `staticlib`
//! output needs to be compiled for `aarch64-apple-ios`/`aarch64-apple-ios-sim`
//! on a Mac with Xcode, which this Windows environment doesn't have. The
//! Android JNI bridge (`jni_bridge`) is the one actually exercised so far;
//! this exists so the iOS side has something correct to link against once
//! that build is possible, per the project's documented Etapa 7 scope.

use std::os::raw::c_uchar;
use std::sync::Mutex;

use crate::Instance;

#[no_mangle]
pub extern "C" fn gba_create() -> *mut Mutex<Instance> {
    Box::into_raw(Box::new(Mutex::new(Instance::new())))
}

/// # Safety
/// `ptr` must be a pointer previously returned by [`gba_create`] and not
/// already destroyed.
#[no_mangle]
pub unsafe extern "C" fn gba_destroy(ptr: *mut Mutex<Instance>) {
    if ptr.is_null() {
        return;
    }
    drop(Box::from_raw(ptr));
}

/// # Safety
/// `ptr` must come from [`gba_create`]; `rom` must point to `len` valid
/// bytes for the duration of this call.
#[no_mangle]
pub unsafe extern "C" fn gba_load_rom(ptr: *mut Mutex<Instance>, rom: *const c_uchar, len: usize) -> bool {
    if ptr.is_null() || rom.is_null() {
        return false;
    }
    let bytes = std::slice::from_raw_parts(rom, len).to_vec();
    let Ok(mut instance) = (*ptr).lock() else { return false };
    instance.load_rom(bytes)
}

/// Loads a user-supplied real BIOS dump for the *next* [`gba_load_rom`]
/// call to use. Must be exactly 16KB — anything else is rejected.
///
/// # Safety
/// `ptr` must come from [`gba_create`]; `bios` must point to `len` valid
/// bytes for the duration of this call.
#[no_mangle]
pub unsafe extern "C" fn gba_load_bios(ptr: *mut Mutex<Instance>, bios: *const c_uchar, len: usize) -> bool {
    if ptr.is_null() || bios.is_null() {
        return false;
    }
    let bytes = std::slice::from_raw_parts(bios, len).to_vec();
    let Ok(mut instance) = (*ptr).lock() else { return false };
    instance.load_bios(bytes)
}

/// # Safety
/// `ptr` must come from [`gba_create`].
#[no_mangle]
pub unsafe extern "C" fn gba_load_test_pattern(ptr: *mut Mutex<Instance>) {
    if ptr.is_null() {
        return;
    }
    if let Ok(mut instance) = (*ptr).lock() {
        instance.load_test_pattern();
    }
}

/// # Safety
/// `ptr` must come from [`gba_create`].
#[no_mangle]
pub unsafe extern "C" fn gba_run_frame(ptr: *mut Mutex<Instance>) -> bool {
    if ptr.is_null() {
        return false;
    }
    let Ok(mut instance) = (*ptr).lock() else { return false };
    instance.run_frame()
}

/// Writes the framebuffer as `SCREEN_WIDTH * SCREEN_HEIGHT` 0xAARRGGBB
/// pixels into `out`.
///
/// # Safety
/// `ptr` must come from [`gba_create`]; `out` must point to at least
/// `gba_core::ppu::SCREEN_WIDTH * gba_core::ppu::SCREEN_HEIGHT` valid,
/// writable `i32` slots.
#[no_mangle]
pub unsafe extern "C" fn gba_get_framebuffer(ptr: *mut Mutex<Instance>, out: *mut i32) {
    if ptr.is_null() || out.is_null() {
        return;
    }
    let Ok(instance) = (*ptr).lock() else { return };
    let len = gba_core::ppu::SCREEN_WIDTH * gba_core::ppu::SCREEN_HEIGHT;
    let slice = std::slice::from_raw_parts_mut(out, len);
    instance.write_framebuffer_argb(slice);
}

/// Writes up to `max_len` interleaved (left, right) i16 PCM samples
/// produced by the most recent [`gba_run_frame`] into `out`, and returns
/// how many were actually written (0 if there's no instance or nothing to
/// hand back). Unlike [`gba_get_framebuffer`]'s fixed frame size, the
/// sample count varies by a sample or two between calls — see
/// `Instance::run_frame` — so callers should size `out` generously (a
/// couple hundred samples above one video frame's worth at whatever rate
/// the core resamples to) and use the returned count, not `max_len`.
///
/// # Safety
/// `ptr` must come from [`gba_create`]; `out` must point to at least
/// `max_len` valid, writable `i16` slots.
#[no_mangle]
pub unsafe extern "C" fn gba_take_audio_samples(ptr: *mut Mutex<Instance>, out: *mut i16, max_len: usize) -> usize {
    if ptr.is_null() || out.is_null() {
        return 0;
    }
    let Ok(mut instance) = (*ptr).lock() else { return 0 };
    let samples = instance.take_audio_samples();
    let n = samples.len().min(max_len);
    let slice = std::slice::from_raw_parts_mut(out, n);
    slice.copy_from_slice(&samples[..n]);
    n
}

/// # Safety
/// `ptr` must come from [`gba_create`] (or be null, in which case this is
/// a no-op).
#[no_mangle]
pub unsafe extern "C" fn gba_set_key(ptr: *mut Mutex<Instance>, key: i32, pressed: bool) {
    if ptr.is_null() {
        return;
    }
    if let Ok(mut instance) = (*ptr).lock() {
        instance.set_key(key, pressed);
    }
}
