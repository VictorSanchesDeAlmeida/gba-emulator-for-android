package expo.modules.gbaemulator

import expo.modules.kotlin.modules.Module
import expo.modules.kotlin.modules.ModuleDefinition

class GbaEmulatorModule : Module() {
  override fun definition() = ModuleDefinition {
    Name("GbaEmulator")

    View(GbaEmulatorView::class) {
      // Base64-encoded ROM bytes. A one-time cost on load (not per-frame),
      // so crossing the bridge here is fine — it's exactly the kind of
      // traffic the "never send the framebuffer over the bridge" rule
      // doesn't apply to.
      Prop("romBase64") { view: GbaEmulatorView, value: String? ->
        view.setRomBase64(value)
      }
      // Base64-encoded real GBA BIOS dump (user-supplied — this project
      // doesn't ship Nintendo's own BIOS). Optional: without it, ROMs boot
      // through gba-core's built-in HLE instead.
      Prop("biosBase64") { view: GbaEmulatorView, value: String? ->
        view.setBiosBase64(value)
      }
      // Development/diagnostic only: renders a fixed colorful scene
      // without needing a ROM, to verify the rendering pipeline itself.
      Prop("testPattern") { view: GbaEmulatorView, value: Boolean ->
        view.setTestPattern(value)
      }
      Prop("running") { view: GbaEmulatorView, value: Boolean ->
        view.setRunning(value)
      }
      // Full set of currently-held button names ("A", "B", "Select",
      // "Start", "Right", "Left", "Up", "Down", "R", "L"). The UI only
      // ever describes "what's held right now" — the view diffs this
      // against its previous set and forwards press/release deltas.
      Prop("pressedButtons") { view: GbaEmulatorView, value: List<String>? ->
        view.setPressedButtons(value ?: emptyList())
      }
    }
  }
}
