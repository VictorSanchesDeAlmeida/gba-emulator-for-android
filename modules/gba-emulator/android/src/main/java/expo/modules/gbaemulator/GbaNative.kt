package expo.modules.gbaemulator

/**
 * Thin JNI declarations matching `native/ffi/src/jni_bridge.rs` exactly —
 * this file has no logic of its own. `ptr` is an opaque handle to a boxed
 * Rust `Instance`; callers own its lifecycle (create once, destroy once).
 */
object GbaNative {
  init {
    System.loadLibrary("gba_ffi")
  }

  external fun create(): Long
  external fun destroy(ptr: Long)
  external fun loadRom(ptr: Long, rom: ByteArray): Boolean
  external fun loadBios(ptr: Long, bios: ByteArray): Boolean
  external fun loadTestPattern(ptr: Long)
  external fun runFrame(ptr: Long): Boolean
  external fun getFrameBuffer(ptr: Long, out: IntArray)
  // Interleaved (left, right) i16 PCM samples produced by the most recent
  // runFrame call. Length varies by a sample or two frame to frame (see
  // `Instance::run_frame` on the Rust side) — always a freshly-sized array,
  // never a fixed-length out-param like getFrameBuffer.
  external fun getAudioBuffer(ptr: Long): ShortArray
  external fun setKey(ptr: Long, key: Int, pressed: Boolean)
}
