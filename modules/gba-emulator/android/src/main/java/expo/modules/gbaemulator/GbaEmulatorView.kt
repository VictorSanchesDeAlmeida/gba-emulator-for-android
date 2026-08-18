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
import java.io.File
import java.util.concurrent.Executors

private const val SCREEN_WIDTH = 240
private const val SCREEN_HEIGHT = 160

// Must match `AUDIO_SAMPLE_RATE_HZ` in native/ffi/src/lib.rs exactly — that's
// the rate the core's audio gets resampled to before it ever reaches this
// view, so this side just has to play it back at the same rate, not choose one.
private const val AUDIO_SAMPLE_RATE_HZ = 32_768

// GBA's real, fixed refresh rate — CPU_CLOCK_HZ / CYCLES_PER_FRAME in
// native/ffi/src/lib.rs, i.e. exactly 16,777,216 / 280,896 Hz. Choreographer
// callbacks are *not* a reliable proxy for this: they fire at whatever the
// host display's actual composition rate is (60Hz normally, but often 90 or
// 120Hz on modern hardware, or throttled to 30Hz under power saving/thermal
// limits — this device measured a sustained 30Hz during testing). Calling
// runFrame() exactly once per callback silently ties emulation speed to
// that host rate instead of the GBA's own: at 30Hz the game runs at half
// speed and produces audio at half the real-time rate, which starves
// AudioTrack's playback (draining at the real, fixed 32,768Hz) into a
// continuous, growing underrun — the actual cause of this project's
// "audio estourando" bug, not a gain/clipping issue. `doFrame` below
// instead tracks real elapsed time and runs as many emulated frames as
// that time is actually worth, so playback speed and audio pacing stay
// correct regardless of the host's callback rate.
private const val GBA_FRAME_NANOS = 1_000_000_000L * 280_896L / 16_777_216L

// Caps how many emulated frames a single doFrame catches up on after a
// stall (e.g. the app was backgrounded, or a GC pause ate several host
// frames) — without this, a long-enough stall would make doFrame try to
// run thousands of frames in one shot, hanging the UI thread trying to
// "catch up" instead of just resuming at normal speed with a brief skip.
private const val MAX_CATCHUP_FRAMES = 4

// How often doFrame checks whether cartridge save memory (SRAM/Flash) has
// changed and, if so, persists it to disk — a real cartridge's save chip
// is battery-backed and just always current, but this emulator only has
// an in-memory copy until something writes it out. ~1s: frequent enough
// that a crash/force-close loses at most a second of progress, infrequent
// enough that the (cheap, but non-zero) byte-compare and occasional disk
// write never contend with the audio/video pacing this same callback owns.
private const val SAVE_CHECK_INTERVAL_FRAMES = 60

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
 * Choreographer-scheduled loop — paced by real elapsed time against the
 * GBA's own fixed frame rate (see [GBA_FRAME_NANOS]), not by raw callback
 * count — no per-frame JS bridge traffic. React only sets props (which
 * ROM to load, whether to run) and lays the view out; everything from
 * "Rust produced a frame" to "pixels on screen" stays native.
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

  // Where this ROM's cartridge save memory lives on disk, chosen by the JS
  // side (typically derived from the ROM's own filename) — this view just
  // reads/writes whatever path it's given, on its own schedule; see
  // maybeLoadRom (restore) and persistSaveData (autosave).
  private var savePath: String? = null
  private var lastPersistedSaveData: ByteArray? = null
  private var saveCheckCounter = 0

  // Autosave writes happen off the Choreographer thread so a slow disk
  // never steals time from audio/video pacing — the same lesson the frame
  // pacing fix above already had to learn once. Single-threaded: save
  // writes are small and infrequent, and strictly ordering them avoids an
  // older write racing a newer one out of order onto disk.
  private val saveExecutor = Executors.newSingleThreadExecutor()

  // Real elapsed time not yet "spent" on an emulated frame — see
  // GBA_FRAME_NANOS's doc comment for why doFrame is paced by this instead
  // of by raw Choreographer callback count.
  private var lastFrameTimeNanos = 0L
  private var accumulatorNanos = 0L

  private val frameCallback = object : Choreographer.FrameCallback {
    override fun doFrame(frameTimeNanos: Long) {
      if (lastFrameTimeNanos != 0L) {
        accumulatorNanos += frameTimeNanos - lastFrameTimeNanos
      }
      lastFrameTimeNanos = frameTimeNanos

      if (running && loaded) {
        var framesRun = 0
        while (accumulatorNanos >= GBA_FRAME_NANOS && framesRun < MAX_CATCHUP_FRAMES) {
          if (!GbaNative.runFrame(nativePtr)) break
          accumulatorNanos -= GBA_FRAME_NANOS
          framesRun++

          val samples = GbaNative.getAudioBuffer(nativePtr)
          if (samples.isNotEmpty()) {
            audioTrack.write(samples, 0, samples.size)
          }
        }
        // A stall much longer than the catch-up cap (app backgrounded,
        // a long GC pause, ...) — resume at normal speed with a brief
        // visible/audible skip instead of ever trying to run the missed
        // time back-to-back.
        if (accumulatorNanos > GBA_FRAME_NANOS * MAX_CATCHUP_FRAMES) {
          accumulatorNanos = 0L
        }
        if (framesRun > 0) {
          GbaNative.getFrameBuffer(nativePtr, pixels)
          bitmap.setPixels(pixels, 0, SCREEN_WIDTH, 0, 0, SCREEN_WIDTH, SCREEN_HEIGHT)
          invalidate()

          saveCheckCounter += framesRun
          if (saveCheckCounter >= SAVE_CHECK_INTERVAL_FRAMES) {
            saveCheckCounter = 0
            persistSaveDataIfChanged()
          }
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

  // Where to read/write this ROM's cartridge save memory. Like
  // biosBase64/romBase64, this re-triggers maybeLoadRom on change — the
  // JS side sets it once, alongside romBase64, when a ROM is first picked,
  // not something that changes mid-session.
  fun setSavePath(path: String?) {
    savePath = path
    maybeLoadRom()
  }

  private fun maybeLoadRom() {
    val rom = pendingRomBytes ?: return
    biosBytes?.let { GbaNative.loadBios(nativePtr, it) }
    loaded = GbaNative.loadRom(nativePtr, rom)
    running = loaded
    lastPersistedSaveData = null
    saveCheckCounter = 0
    if (loaded) {
      savePath?.let { path ->
        val file = File(path)
        if (file.exists()) {
          val data = file.readBytes()
          GbaNative.loadSaveData(nativePtr, data)
          lastPersistedSaveData = data
        }
      }
    }
  }

  /**
   * Writes current cartridge save memory to [savePath] if it differs from
   * what's already there — called periodically from [doFrame] and once
   * more (synchronously) from [onDetachedFromWindow]. A no-op if there's
   * no save path, no ROM loaded, or the cartridge has no save chip
   * (`getSaveData` returns empty in that case).
   */
  private fun persistSaveDataIfChanged(sync: Boolean = false) {
    val path = savePath ?: return
    if (!loaded) return
    val data = GbaNative.getSaveData(nativePtr)
    if (data.isEmpty() || data.contentEquals(lastPersistedSaveData)) return
    lastPersistedSaveData = data
    val write = {
      try {
        val file = File(path)
        file.parentFile?.mkdirs()
        file.writeBytes(data)
      } catch (_: Exception) {
        // Best-effort: a failed autosave shouldn't crash gameplay. The
        // next successful write (or the final one on close) still lands.
      }
    }
    if (sync) write() else saveExecutor.execute(write)
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
    // Synchronous: the view (and possibly the process) may not survive
    // long enough for a background-queued write to run.
    persistSaveDataIfChanged(sync = true)
    saveExecutor.shutdown()
    Choreographer.getInstance().removeFrameCallback(frameCallback)
    audioTrack.stop()
    audioTrack.release()
  }
}
