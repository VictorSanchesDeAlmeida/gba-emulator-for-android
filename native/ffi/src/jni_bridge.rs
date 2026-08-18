//! JNI exports backing the Android Kotlin object `GbaNative`
//! (`modules/gba-emulator/android/.../GbaNative.kt`). Function names must
//! match `Java_<package>_<Class>_<method>` exactly — Kotlin's `external
//! fun` declarations are what JNI resolves these against.

use std::sync::Mutex;

use jni::objects::{JByteArray, JClass, JIntArray, JObject};
use jni::sys::{jboolean, jint, jlong, jshortArray, JNI_FALSE};
use jni::JNIEnv;

use crate::Instance;

#[no_mangle]
pub extern "system" fn Java_expo_modules_gbaemulator_GbaNative_create(_env: JNIEnv, _class: JClass) -> jlong {
    Box::into_raw(Box::new(Mutex::new(Instance::new()))) as jlong
}

#[no_mangle]
pub extern "system" fn Java_expo_modules_gbaemulator_GbaNative_destroy(_env: JNIEnv, _class: JClass, ptr: jlong) {
    if ptr == 0 {
        return;
    }
    unsafe {
        drop(Box::from_raw(ptr as *mut Mutex<Instance>));
    }
}

#[no_mangle]
pub extern "system" fn Java_expo_modules_gbaemulator_GbaNative_loadRom<'local>(
    env: JNIEnv<'local>,
    _class: JClass<'local>,
    ptr: jlong,
    rom: JByteArray<'local>,
) -> jboolean {
    let Ok(bytes) = env.convert_byte_array(&rom) else {
        return JNI_FALSE;
    };
    with_instance(ptr, |instance| instance.load_rom(bytes) as jboolean).unwrap_or(JNI_FALSE)
}

#[no_mangle]
pub extern "system" fn Java_expo_modules_gbaemulator_GbaNative_loadBios<'local>(
    env: JNIEnv<'local>,
    _class: JClass<'local>,
    ptr: jlong,
    bios: JByteArray<'local>,
) -> jboolean {
    let Ok(bytes) = env.convert_byte_array(&bios) else {
        return JNI_FALSE;
    };
    with_instance(ptr, |instance| instance.load_bios(bytes) as jboolean).unwrap_or(JNI_FALSE)
}

#[no_mangle]
pub extern "system" fn Java_expo_modules_gbaemulator_GbaNative_loadTestPattern(_env: JNIEnv, _class: JClass, ptr: jlong) {
    with_instance(ptr, |instance| instance.load_test_pattern());
}

#[no_mangle]
pub extern "system" fn Java_expo_modules_gbaemulator_GbaNative_runFrame(_env: JNIEnv, _class: JClass, ptr: jlong) -> jboolean {
    with_instance(ptr, |instance| instance.run_frame() as jboolean).unwrap_or(JNI_FALSE)
}

#[no_mangle]
pub extern "system" fn Java_expo_modules_gbaemulator_GbaNative_getFrameBuffer<'local>(
    env: JNIEnv<'local>,
    _class: JClass<'local>,
    ptr: jlong,
    out: JIntArray<'local>,
) {
    let len = match env.get_array_length(&out) {
        Ok(len) => len as usize,
        Err(_) => return,
    };
    let mut pixels = vec![0i32; len];
    with_instance(ptr, |instance| instance.write_framebuffer_argb(&mut pixels));
    let _ = env.set_int_array_region(&out, 0, &pixels);
}

/// Returns the interleaved (left, right) i16 PCM samples produced by the
/// most recent `runFrame` call, as a freshly-allocated Java `ShortArray` —
/// unlike `getFrameBuffer`'s fixed-size out-parameter, this varies in
/// length by a sample or two between calls (see `Instance::run_frame`), so
/// there's no fixed size for the caller to preallocate against. Returns an
/// empty array (never null) if there's no instance or nothing to hand back.
#[no_mangle]
pub extern "system" fn Java_expo_modules_gbaemulator_GbaNative_getAudioBuffer<'local>(
    env: JNIEnv<'local>,
    _class: JClass<'local>,
    ptr: jlong,
) -> jshortArray {
    let samples = with_instance(ptr, |instance| instance.take_audio_samples()).unwrap_or_default();
    let Ok(array) = env.new_short_array(samples.len() as jint) else {
        return std::ptr::null_mut();
    };
    let _ = env.set_short_array_region(&array, 0, &samples);
    JObject::from(array).into_raw()
}

#[no_mangle]
pub extern "system" fn Java_expo_modules_gbaemulator_GbaNative_setKey(
    _env: JNIEnv,
    _class: JClass,
    ptr: jlong,
    key: jint,
    pressed: jboolean,
) {
    with_instance(ptr, |instance| instance.set_key(key, pressed != 0));
}

fn with_instance<T>(ptr: jlong, f: impl FnOnce(&mut Instance) -> T) -> Option<T> {
    if ptr == 0 {
        return None;
    }
    let instance = unsafe { &*(ptr as *const Mutex<Instance>) };
    let mut guard = instance.lock().ok()?;
    Some(f(&mut guard))
}
