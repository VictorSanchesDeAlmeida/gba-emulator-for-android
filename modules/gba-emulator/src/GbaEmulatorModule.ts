import { NativeModule, requireNativeModule } from 'expo';

declare class GbaEmulatorModule extends NativeModule<{}> {}

export default requireNativeModule<GbaEmulatorModule>('GbaEmulator');
