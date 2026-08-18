import { registerWebModule, NativeModule } from 'expo';

// GbaEmulatorModule is not available on the web platform.
class GbaEmulatorModule extends NativeModule<{}> {}

export default registerWebModule(GbaEmulatorModule, 'GbaEmulatorModule');
