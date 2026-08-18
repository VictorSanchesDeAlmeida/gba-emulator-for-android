import { requireNativeView } from "expo";
import * as React from "react";

import { GbaEmulatorViewProps } from "./GbaEmulator.types";

const NativeView: React.ComponentType<GbaEmulatorViewProps> = requireNativeView("GbaEmulator");

export default function GbaEmulatorView(props: GbaEmulatorViewProps) {
  return <NativeView {...props} />;
}
