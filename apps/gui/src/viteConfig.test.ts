// @vitest-environment node

import { describe, expect, it } from "vitest";
import config from "../vite.config";

describe("vite config", () => {
  it("uses relative asset URLs for native file:// palette loading", () => {
    expect(config.base).toBe("./");
  });
});
