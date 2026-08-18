const { getDefaultConfig } = require("expo/metro-config");

/** @type {import('expo/metro-config').MetroConfig} */
const config = getDefaultConfig(__dirname);

// Lets `require("../../assets/bios/gba_bios.bin")` resolve as a bundled
// binary asset (see src/app/index.tsx) instead of Metro trying to parse it
// as source.
config.resolver.assetExts.push("bin");

module.exports = config;
