import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import { fileURLToPath } from "node:url";

const host = process.env.TAURI_DEV_HOST;

// https://vite.dev/config/
export default defineConfig(async ({ mode }) => ({
  plugins: [react()],

  // The Android onboarding wizard's "recap" screens (art recapping the 4
  // native Android permission screens, see androidStepSequence in
  // OnboardingWizard.jsx) don't exist on desktop's step sequence — but
  // desktop and Android build from the same source and, before this
  // alias, the same `npm run build` output, so those images shipped in
  // every desktop installer too. This alias swaps in a stub with no image
  // imports for any build that isn't `--mode android` (see
  // recapAssets.stub.js / recapAssets.android.js), so Vite's module graph
  // never resolves those assets for a desktop build in the first place —
  // a runtime IS_ANDROID check alone can't do this, since dynamic
  // import() targets are still added to the build's chunk graph even
  // inside a branch that's provably dead at runtime.
  resolve: {
    alias: {
      "@recap-assets": fileURLToPath(
        new URL(
          mode === "android"
            ? "./src/onboarding/recapAssets.android.js"
            : "./src/onboarding/recapAssets.stub.js",
          import.meta.url,
        ),
      ),
    },
  },

  // Vite options tailored for Tauri development and only applied in `tauri dev` or `tauri build`
  //
  // 1. prevent Vite from obscuring rust errors
  clearScreen: false,
  // 2. tauri expects a fixed port, fail if that port is not available
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1421,
        }
      : undefined,
    watch: {
      // 3. tell Vite to ignore watching `src-tauri`
      ignored: ["**/src-tauri/**"],
    },
  },
}));
