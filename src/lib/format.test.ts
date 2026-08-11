import { describe, expect, it } from "vitest";

import { formatDuration, formatElapsed, formatStartedAt } from "./format";

describe("formatDuration", () => {
  it("formats sub-hour durations as mm:ss", () => {
    expect(formatDuration(65)).toBe("1:05");
    expect(formatDuration(0)).toBe("0:00");
  });

  it("formats hour-plus durations as h:mm:ss", () => {
    expect(formatDuration(3661)).toBe("1:01:01");
  });

  it("rounds fractional seconds", () => {
    expect(formatDuration(59.6)).toBe("1:00");
  });
});

describe("formatElapsed", () => {
  it("converts milliseconds to duration label", () => {
    expect(formatElapsed(65_000)).toBe("1:05");
  });
});

describe("formatStartedAt", () => {
  it("returns the original string when parsing fails", () => {
    expect(formatStartedAt("not-a-date")).toBe("not-a-date");
  });

  it("formats a valid ISO timestamp", () => {
    const label = formatStartedAt("2026-08-07T17:30:00.000Z");
    expect(label).toMatch(/Aug/);
    expect(label).toMatch(/7/);
  });
});
