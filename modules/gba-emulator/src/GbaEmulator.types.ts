import { ViewProps } from "react-native";

/** Matches `gba_core::joypad::Button` exactly — same names, same order. */
export type GbaButton = "A" | "B" | "Select" | "Start" | "Right" | "Left" | "Up" | "Down" | "R" | "L";

export type GbaEmulatorViewProps = {
  /** Base64-encoded ROM bytes. Setting this (re)loads and starts the ROM. */
  romBase64?: string;
  /**
   * Base64-encoded real GBA BIOS dump (user-supplied — this project can't
   * legally ship Nintendo's own BIOS). Optional: without it, ROMs boot
   * through the emulator core's built-in HLE, which most homebrew ROMs
   * tolerate but most commercial ROMs don't. Applied on the next ROM load,
   * so set this before (or alongside) `romBase64`.
   */
  biosBase64?: string;
  /**
   * Renders a fixed colorful scene without needing a ROM. Development
   * aid only — verifies the native rendering pipeline independently of
   * whether a given ROM's own boot sequence ever draws anything.
   */
  testPattern?: boolean;
  /** Pauses/resumes the emulation loop. Defaults to true once a ROM or the test pattern loads. */
  running?: boolean;
  /**
   * Every button currently held, e.g. from `onPressIn`/`onPressOut` on a
   * D-Pad. Declarative by design — the UI only ever describes "what's
   * held right now"; the native view computes press/release deltas.
   */
  pressedButtons?: GbaButton[];
} & ViewProps;
