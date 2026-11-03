import { describe, expect, it } from "vitest";
import { cls, fmtBytes, fmtDuration, fmtRate } from "./format";

describe("fmtBytes", () => {
  it("uses binary units", () => {
    expect(fmtBytes(0)).toBe("0 B");
    expect(fmtBytes(512)).toBe("512 B");
    expect(fmtBytes(1536)).toBe("1.50 KiB");
    expect(fmtBytes(20 * 1024 * 1024)).toBe("20.0 MiB");
    expect(fmtBytes(3 * 1024 ** 4)).toBe("3.00 TiB");
  });

  it("treats junk as zero", () => {
    expect(fmtBytes(-5)).toBe("0 B");
    expect(fmtBytes(Number.NaN)).toBe("0 B");
  });

  it("formats rates", () => {
    expect(fmtRate(2048)).toBe("2.00 KiB/s");
  });
});

describe("fmtDuration", () => {
  it("shows the two largest units", () => {
    expect(fmtDuration(42)).toBe("42s");
    expect(fmtDuration(125)).toBe("2m 5s");
    expect(fmtDuration(3 * 3600 + 7 * 60 + 9)).toBe("3h 7m");
    expect(fmtDuration(2 * 86400 + 5 * 3600)).toBe("2d 5h");
  });

  it("rejects negative durations", () => {
    expect(fmtDuration(-1)).toBe("—");
  });
});

describe("cls", () => {
  it("drops falsy parts", () => {
    expect(cls("a", false, null, undefined, "b")).toBe("a b");
  });
});
