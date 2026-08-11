import { describe, expect, it } from "vitest";

import { warningLabelFor } from "./warnings";

describe("warningLabelFor", () => {
  it("maps known warning codes to user-facing copy", () => {
    expect(warningLabelFor("source_silent")).toContain("No audio detected");
    expect(warningLabelFor("disk_space_low")).toContain("Disk space is low");
  });

  it("passes through unknown codes unchanged", () => {
    expect(warningLabelFor("custom_code")).toBe("custom_code");
  });
});
