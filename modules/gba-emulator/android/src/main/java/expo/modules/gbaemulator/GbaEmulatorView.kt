package expo.modules.gbaemulator

import android.content.Context
import android.graphics.Bitmap
import android.graphics.Canvas
import android.graphics.Paint
import android.graphics.Rect
import android.media.AudioAttributes
import android.media.AudioFormat
import android.media.AudioTrack
import android.util.Base64
import android.view.Choreographer
import expo.modules.kotlin.AppContext
import expo.modules.kotlin.views.ExpoView

private const val SCREEN_WIDTH = 240
private const val SCREEN_HEIGHT = 160

// Must match `AUDIO_SAMPLE_RATE_HZ` in native/ffi/src/lib.rs exactly — that's
// the rate the core's audio gets resampled to before it ever reaches this
// view, so this side just has to play it back at the same rate, not choose one.
private const val AUDIO_SAMPLE_RATE_HZ = 32_768

/**
 * Button name -> numeric ID. Must match `gba_core::joypad::Button::from_index`
 * exactly — this mapping is a cross-language contract, not a local detail.
 */
private val BUTTON_IDS = mapOf(
  "A" to 0,
  "B" to 1,
  "Select" to 2,
  "Start" to 3,
  "Right" to 4,
  "Left" to 5,
  "Up" to 6,
  "Down" to 7,
  "R" to 8,
  "L" to 9,
)

/**
 * Renders the GBA framebuffer directly to a Canvas, driven by its own
 * Choreographer-scheduled loop (~60Hz, matching the console's own
 * refresh rate) — no per-frame JS bridge traffic. React only sets props
 * (which ROM to load, whether to run) and lays the view out; everything
 * from "Rust produced a frame" to "pixels on screen" stays native.
 */
class GbaEmulatorView(context: Context, appContext: AppContext) : ExpoView(context, appContext) {
  private val nativePtr: Long = GbaNative.create()
  private val pixels = IntArray(SCREEN_WIDTH * SCREEN_HEIGHT)
  private val bitmap = Bitmap.createBitmap(SCREEN_WIDTH, SCREEN_HEIGHT, Bitmap.Config.ARGB_8888)
  private val paint = Paint().apply { isFilterBitmap = false } // nearest-neighbor: never blur GBA pixels
  private val destRect = Rect()

  // Streaming playback for the PCM samples GbaNative.getAudioBuffer hands
  // back each frame. Left in PLAYING state for the view's whole lifetime
  // (matching the video side: doFrame runs continuously too) — when
  // `running` is false, doFrame simply never writes new samples to it,
  // which just leaves it silently underrunning rather than needing an
  // explicit pause/resume dance.
  private val audioTrack = AudioTrack.Builder()
    .setAudioAttributes(
      AudioAttributes.Builder()
        .setUsage(AudioAttributes.USAGE_GAME)
        .setContentType(AudioAttributes.CONTENT_TYPE_SONIFICATION)
        .build()
    )
    .setAudioFormat(
      AudioFormat.Builder()
        .setSampleRate(AUDIO_SAMPLE_RATE_HZ)
        .setEncoding(AudioFormat.ENCODING_PCM_16BIT)
        .setChannelMask(AudioFormat.CHANNEL_OUT_STEREO)
        .build()
    )
    .setBufferSizeInBytes(
      // A few frames of headroom above the platform minimum so a stray
      // Choreographer hiccup doesn't immediately starve playback.
      AudioTrack.getMinBufferSize(AUDIO_SAMPLE_RATE_HZ, AudioFormat.CHANNEL_OUT_STEREO, AudioFormat.ENCODING_PCM_16BIT) * 2
    )
    .setTransferMode(AudioTrack.MODE_STREAM)
    .build()

  private var running = false
  private var loaded = false
  private val heldButtons = mutableSetOf<String>()

  // A user-supplied real BIOS dump, base64-decoded and re-applied to the
  // native instance immediately before every ROM (re)load — see
  // maybeLoadRom. Kept here (not fired into GbaNative just once) because
  // prop update order between `biosBase64` and `romBase64` isn't
  // guaranteed: on a view's very first mount both can arrive together, and
  // an already-running ROM's Emulator can't retroactively pick up a BIOS
  // that arrives after it. maybeLoadRom() re-loads the ROM from scratch
  // whenever either prop changes, so whichever order they arrive in, the
  // ROM ends up (re)loaded with whatever BIOS is currently known.
  private var biosBytes: ByteArray? = null
  private var pendingRomBytes: ByteArray? = null

  private val frameCallback = object : Choreographer.FrameCallback {
    override fun doFrame(frameTimeNanos: Long) {
      if (running && loaded && GbaNative.runFrame(nativePtr)) {
        GbaNative.getFrameBuffer(nativePtr, pixels)
        bitmap.setPixels(pixels, 0, SCREEN_WIDTH, 0, 0, SCREEN_WIDTH, SCREEN_HEIGHT)
        invalidate()

        val samples = GbaNative.getAudioBuffer(nativePtr)
        if (samples.isNotEmpty()) {
          audioTrack.write(samples, 0, samples.size)
        }
      }
      Choreographer.getInstance().postFrameCallback(this)
    }
  }

  init {
    setWillNotDraw(false) // ViewGroups skip onDraw by default; we need it
    audioTrack.play()
    Choreographer.getInstance().postFrameCallback(frameCallback)
  }

  fun setBiosBase64(base64: String?) {
    biosBytes = if (base64.isNullOrEmpty()) null else Base64.decode(base64, Base64.DEFAULT)
    maybeLoadRom()
  }

  fun setRomBase64(base64: String?) {
    pendingRomBytes = if (base64.isNullOrEmpty()) null else Base64.decode(base64, Base64.DEFAULT)
    maybeLoadRom()
  }

  private fun maybeLoadRom() {
    val rom = pendingRomBytes ?: return
    biosBytes?.let { GbaNative.loadBios(nativePtr, it) }
    loaded = GbaNative.loadRom(nativePtr, rom)
    running = loaded
  }

  fun setTestPattern(enabled: Boolean) {
    if (!enabled) return
    GbaNative.loadTestPattern(nativePtr)
    loaded = true
    running = true
  }

  fun setRunning(value: Boolean) {
    running = value
  }

  /**
   * Full set of currently-held button names, sent declaratively (not as
   * imperative press/release calls) so React only ever needs to describe
   * "what's held right now" from its own touch state — this diffs against
   * the previous set and forwards just the press/release deltas to Rust.
   */
  fun setPressedButtons(names: List<String>) {
    val next = names.toSet()
    for (name in next - heldButtons) {
      BUTTON_IDS[name]?.let { GbaNative.setKey(nativePtr, it, true) }
    }
    for (name in heldButtons - next) {
      BUTTON_IDS[name]?.let { GbaNative.setKey(nativePtr, it, false) }
    }
    heldButtons.clear()
    heldButtons.addAll(next)
  }

  override fun onDraw(canvas: Canvas) {
    super.onDraw(canvas)
    // Integer scale to the largest 240x160 rectangle that fits, centered —
    // preserves the original aspect ratio instead of stretching to fill.
    val scale = minOf(width / SCREEN_WIDTH, height / SCREEN_HEIGHT).coerceAtLeast(1)
    val destWidth = SCREEN_WIDTH * scale
    val destHeight = SCREEN_HEIGHT * scale
    val left = (width - destWidth) / 2
    val top = (height - destHeight) / 2
    destRect.set(left, top, left + destWidth, top + destHeight)
    canvas.drawBitmap(bitmap, null, destRect, paint)
  }

  override fun onDetachedFromWindow() {
    super.onDetachedFromWindow()
    Choreographer.getInstance().removeFrameCallback(frameCallback)
    audioTrack.stop()
    audioTrack.release()
  }
}
