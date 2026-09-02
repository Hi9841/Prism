import { defineConfig } from "vitest/config";

export default defineConfig({
  test: {
    include: ["landing/**/*.test.ts"],
    environment: "node",
  },
});
