import { defineConfig } from "vitest/config";

export default defineConfig({
  test: {
    include: ["tests/**/*.test.ts"],
    // Nothing here may touch a network. Mocks and pure crypto only.
    testTimeout: 30_000,
  },
});
