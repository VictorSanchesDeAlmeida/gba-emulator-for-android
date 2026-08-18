import { Asset } from "expo-asset";
import * as DocumentPicker from "expo-document-picker";
import * as FileSystem from "expo-file-system/legacy";
import { useEffect, useState } from "react";
import {
  Alert,
  Dimensions,
  Pressable,
  StyleSheet,
  Text,
  View,
} from "react-native";
import { SafeAreaView } from "react-native-safe-area-context";

import { GbaButton } from "../../modules/gba-emulator/src/GbaEmulator.types";
import GbaEmulatorView from "../../modules/gba-emulator/src/GbaEmulatorView";

const ACCENT = "#7C6BAE"; // GBA-indigo
const SCREEN_ASPECT = 240 / 160;
const SCREEN_WIDTH = Dimensions.get("screen").width - 30;

// This project can't ship Nintendo's own BIOS, so it bundles
// Cult-of-GBA/BIOS instead — an independent, MIT-licensed, from-scratch
// replacement (see assets/bios/LICENSE) — so the app never has to ask the
// user for a dump of their own.
// eslint-disable-next-line @typescript-eslint/no-require-imports -- .bin isn't in eslint-config-expo's asset allowlist, but metro.config.js registers it as one (see there)
const BUNDLED_BIOS = require("../../assets/bios/gba_bios.bin");

/** Held-button set for a Pressable's onPressIn/onPressOut, one button at a time. */
function useHeldButtons() {
  const [held, setHeld] = useState<GbaButton[]>([]);
  const press = (button: GbaButton) =>
    setHeld((prev) => (prev.includes(button) ? prev : [...prev, button]));
  const release = (button: GbaButton) =>
    setHeld((prev) => prev.filter((b) => b !== button));
  return { held, press, release };
}

function DPadButton({
  label,
  style,
  onPress,
  onRelease,
}: {
  label: string;
  style: object;
  onPress: () => void;
  onRelease: () => void;
}) {
  return (
    <Pressable
      style={({ pressed }) => [
        styles.dpadCell,
        style,
        pressed && styles.pressedDark,
      ]}
      onPressIn={onPress}
      onPressOut={onRelease}
    >
      <Text style={styles.dpadLabel}>{label}</Text>
    </Pressable>
  );
}

function RoundButton({
  label,
  onPress,
  onRelease,
}: {
  label: string;
  onPress: () => void;
  onRelease: () => void;
}) {
  return (
    <Pressable
      style={({ pressed }) => [
        styles.roundButton,
        pressed && styles.pressedAccent,
      ]}
      onPressIn={onPress}
      onPressOut={onRelease}
    >
      <Text style={styles.roundButtonLabel}>{label}</Text>
    </Pressable>
  );
}

/**
 * Where a picked ROM's cartridge save memory should live on disk, derived
 * from the ROM's own filename (swapping its extension for `.sav`) so the
 * same save is found again next time the same ROM is picked — the usual
 * convention standalone GBA emulators use. Lives in `documentDirectory`
 * (survives app restarts, not cleared like cache) under `saves/`; the
 * native side creates that directory itself on first write.
 */
function savePathFor(romFileName: string): string | undefined {
  const documentDir = FileSystem.documentDirectory;
  if (!documentDir) return undefined;
  // The native side reads/writes this with a plain java.io.File, which
  // needs a filesystem path, not a `file://` URI.
  const rawDir = documentDir.replace(/^file:\/\//, "");
  const baseName = romFileName.replace(/\.[^./]+$/, "");
  return `${rawDir}saves/${baseName}.sav`;
}

export default function HomeScreen() {
  const [romBase64, setRomBase64] = useState<string | undefined>(undefined);
  const [testPattern, setTestPattern] = useState(false);
  const [biosBase64, setBiosBase64] = useState<string | undefined>(undefined);
  const [savePath, setSavePath] = useState<string | undefined>(undefined);
  const { held, press, release } = useHeldButtons();

  const isRunning = Boolean(romBase64) || testPattern;

  // Load the bundled open-source BIOS replacement once, on first mount —
  // no user action needed. If this somehow fails, biosBase64 just stays
  // undefined and the core's built-in (less game-compatible) HLE BIOS
  // takes over instead of the app being unable to run anything.
  useEffect(() => {
    (async () => {
      try {
        const asset = Asset.fromModule(BUNDLED_BIOS);
        await asset.downloadAsync();
        if (!asset.localUri) return;
        const base64 = await FileSystem.readAsStringAsync(asset.localUri, {
          encoding: FileSystem.EncodingType.Base64,
        });
        setBiosBase64(base64);
      } catch (err) {
        console.warn("Failed to load bundled BIOS", err);
      }
    })();
  }, []);

  async function pickRom() {
    const result = await DocumentPicker.getDocumentAsync({
      copyToCacheDirectory: true,
    });
    if (result.canceled) return;
    try {
      const asset = result.assets[0];
      const base64 = await FileSystem.readAsStringAsync(asset.uri, {
        encoding: FileSystem.EncodingType.Base64,
      });
      setRomBase64(base64);
      setSavePath(savePathFor(asset.name));
      setTestPattern(false);
    } catch (err) {
      Alert.alert("Failed to read ROM", String(err));
    }
  }

  function closeGame() {
    setRomBase64(undefined);
    setSavePath(undefined);
    setTestPattern(false);
  }

  return (
    <SafeAreaView style={styles.container}>
      <View style={styles.header}>
        <Text style={styles.title}>GBA EMULATOR</Text>
        {isRunning && (
          <Pressable onPress={closeGame} hitSlop={12}>
            <Text style={styles.closeAction}>Close</Text>
          </Pressable>
        )}
      </View>
      <View style={styles.screenFrame}>
        {isRunning ? (
          <GbaEmulatorView
            style={styles.screen}
            testPattern={testPattern}
            romBase64={romBase64}
            biosBase64={biosBase64}
            savePath={savePath}
            pressedButtons={held}
            running
          />
        ) : (
          <View style={styles.screenPlaceholder}>
            <Text style={styles.screenPlaceholderText}>No ROM loaded</Text>
          </View>
        )}
      </View>

      {isRunning ? (
        <View style={styles.controlsArea}>
          <View style={styles.controls}>
            <View style={styles.dpad}>
              <DPadButton
                label="▲"
                style={styles.dpadUp}
                onPress={() => press("Up")}
                onRelease={() => release("Up")}
              />
              <DPadButton
                label="◀"
                style={styles.dpadLeft}
                onPress={() => press("Left")}
                onRelease={() => release("Left")}
              />
              <DPadButton
                label="▶"
                style={styles.dpadRight}
                onPress={() => press("Right")}
                onRelease={() => release("Right")}
              />
              <DPadButton
                label="▼"
                style={styles.dpadDown}
                onPress={() => press("Down")}
                onRelease={() => release("Down")}
              />
            </View>

            <View style={styles.abGroup}>
              <RoundButton
                label="B"
                onPress={() => press("B")}
                onRelease={() => release("B")}
              />
              <RoundButton
                label="A"
                onPress={() => press("A")}
                onRelease={() => release("A")}
              />
            </View>
          </View>

          <View style={styles.miscRow}>
            <Pressable
              style={styles.miscButton}
              onPressIn={() => press("Select")}
              onPressOut={() => release("Select")}
            >
              <Text style={styles.miscButtonLabel}>SELECT</Text>
            </Pressable>
            <Pressable
              style={styles.miscButton}
              onPressIn={() => press("Start")}
              onPressOut={() => release("Start")}
            >
              <Text style={styles.miscButtonLabel}>START</Text>
            </Pressable>
          </View>
        </View>
      ) : (
        <View style={styles.homeActions}>
          <Pressable style={styles.primaryButton} onPress={pickRom}>
            <Text style={styles.primaryButtonLabel}>Load ROM</Text>
          </Pressable>
          <Pressable
            style={styles.secondaryButton}
            onPress={() => setTestPattern(true)}
          >
            <Text style={styles.secondaryButtonLabel}>Test pattern (dev)</Text>
          </Pressable>

          <Text style={styles.sectionLabel}>Recent ROMs</Text>
          <View style={styles.recentEmpty}>
            <Text style={styles.recentEmptyText}>Nothing here yet</Text>
          </View>
        </View>
      )}
    </SafeAreaView>
  );
}

const styles = StyleSheet.create({
  container: {
    flex: 1,
    backgroundColor: "#0B0B0E",
    alignItems: "center",
  },
  header: {
    flexDirection: "row",
    alignItems: "center",
    justifyContent: "space-between",
    alignSelf: "stretch",
    paddingHorizontal: 24,
    paddingTop: 12,
  },
  title: {
    color: "#fff",
    fontSize: 18,
    fontWeight: "700",
    letterSpacing: 2,
  },
  closeAction: {
    color: ACCENT,
    fontSize: 14,
    fontWeight: "600",
  },
  screenFrame: {
    marginTop: 24,
    padding: 10,
    borderRadius: 16,
    backgroundColor: "#1A1A20",
    borderWidth: 1,
    borderColor: "#2A2A32",
  },
  screen: {
    width: SCREEN_WIDTH,
    height: SCREEN_WIDTH / SCREEN_ASPECT,
    backgroundColor: "#000",
    borderRadius: 4,
  },
  screenPlaceholder: {
    width: SCREEN_WIDTH,
    height: SCREEN_WIDTH / SCREEN_ASPECT,
    backgroundColor: "#000",
    borderRadius: 4,
    alignItems: "center",
    justifyContent: "center",
  },
  screenPlaceholderText: {
    color: "#4A4A55",
    fontSize: 13,
  },
  homeActions: {
    marginTop: 40,
    alignItems: "center",
    gap: 12,
    alignSelf: "stretch",
    paddingHorizontal: 32,
  },
  primaryButton: {
    backgroundColor: ACCENT,
    paddingHorizontal: 40,
    paddingVertical: 14,
    borderRadius: 10,
    alignSelf: "stretch",
    alignItems: "center",
  },
  primaryButtonLabel: {
    color: "#fff",
    fontSize: 16,
    fontWeight: "700",
  },
  secondaryButton: {
    paddingVertical: 8,
  },
  secondaryButtonLabel: {
    color: "#5A5A66",
    fontSize: 12,
  },
  sectionLabel: {
    marginTop: 28,
    alignSelf: "flex-start",
    color: "#7A7A88",
    fontSize: 12,
    fontWeight: "700",
    letterSpacing: 1,
  },
  recentEmpty: {
    alignSelf: "stretch",
    borderRadius: 10,
    borderWidth: 1,
    borderColor: "#22222A",
    borderStyle: "dashed",
    paddingVertical: 24,
    alignItems: "center",
  },
  recentEmptyText: {
    color: "#4A4A55",
    fontSize: 13,
  },
  controlsArea: {
    alignItems: "center",
    alignSelf: "stretch",
    gap: 28,
    marginTop: 56,
  },
  controls: {
    flexDirection: "row",
    alignItems: "center",
    justifyContent: "center",
    gap: 56,
  },
  dpad: {
    width: 132,
    height: 132,
  },
  dpadCell: {
    position: "absolute",
    width: 44,
    height: 44,
    backgroundColor: "#1E1E24",
    alignItems: "center",
    justifyContent: "center",
  },
  dpadUp: { top: 0, left: 44, borderTopLeftRadius: 8, borderTopRightRadius: 8 },
  dpadDown: {
    top: 88,
    left: 44,
    borderBottomLeftRadius: 8,
    borderBottomRightRadius: 8,
  },
  dpadLeft: {
    top: 44,
    left: 0,
    borderTopLeftRadius: 8,
    borderBottomLeftRadius: 8,
  },
  dpadRight: {
    top: 44,
    left: 88,
    borderTopRightRadius: 8,
    borderBottomRightRadius: 8,
  },
  dpadLabel: {
    color: "#8A8A96",
    fontSize: 16,
  },
  pressedDark: {
    backgroundColor: "#2E2E38",
  },
  abGroup: {
    flexDirection: "row",
    gap: 16,
    transform: [{ translateY: 20 }],
  },
  roundButton: {
    width: 56,
    height: 56,
    borderRadius: 28,
    backgroundColor: "#3A2E5C",
    alignItems: "center",
    justifyContent: "center",
  },
  roundButtonLabel: {
    color: "#fff",
    fontSize: 18,
    fontWeight: "700",
  },
  pressedAccent: {
    backgroundColor: ACCENT,
  },
  miscRow: {
    flexDirection: "row",
    gap: 20,
  },
  miscButton: {
    paddingHorizontal: 18,
    paddingVertical: 8,
    borderRadius: 14,
    backgroundColor: "#1E1E24",
    transform: [{ rotate: "-12deg" }],
  },
  miscButtonLabel: {
    color: "#8A8A96",
    fontSize: 11,
    fontWeight: "700",
  },
});
