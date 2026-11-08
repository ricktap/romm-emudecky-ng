import { defineConfig } from "vitest/config";

export default defineConfig({
  test: {
    environment: "happy-dom",
    // Node >= 25 ships a built-in global `localStorage` (Web Storage) that wins over
    // happy-dom's and has no usable methods without `--localstorage-file`, breaking
    // every test that touches storage. Turn Node's off in the test workers so
    // happy-dom's Storage is used on every Node version (the flag is accepted
    // since Node 22, so it is a no-op on the pinned Node 24).
    execArgv: ["--no-experimental-webstorage"],
    globals: true,
    setupFiles: ["./src/test-setup.ts"],
    include: ["src/**/*.{test,spec}.{ts,tsx}"],
    coverage: {
      provider: "v8",
      reporter: ["text", "lcov"],
      include: ["src/**/*.{ts,tsx}"],
      exclude: [
        "src/**/*.{test,spec}.{ts,tsx}",
        "src/test-setup.ts",
        "src/test-utils/**",
        // Aligned with sonar-project.properties `sonar.coverage.exclusions`.
        "src/types/**",
        "src/index.tsx",
        "src/patches/**",
        "src/utils/styleInjector.ts",
      ],
    },
  },
});
