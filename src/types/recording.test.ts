import { describe, expect, it } from "vitest";

import { isAppError } from "./recording";

describe("isAppError", () => {
  it("accepts objects with code and message", () => {
    expect(isAppError({ code: "DISK_FULL", message: "No space" })).toBe(true);
  });

  it("rejects non-objects and partial shapes", () => {
    expect(isAppError(null)).toBe(false);
    expect(isAppError("DISK_FULL")).toBe(false);
    expect(isAppError({ code: "DISK_FULL" })).toBe(false);
  });
});
