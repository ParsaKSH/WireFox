import { describe, expect, it } from "vitest";
import { idError, intError } from "./validate";

describe("idError", () => {
  it("accepts what the core accepts", () => {
    for (const id of ["office-1", "a", "x.y_z", ".hidden", "a..b", "a".repeat(64)]) {
      expect(idError(id)).toBeUndefined();
    }
  });

  it("rejects names made only of dots", () => {
    for (const id of [".", "..", "..."]) expect(idError(id)).toBeDefined();
  });

  it("rejects other characters and long names", () => {
    for (const id of ["a b", "a/b", "ü", "a".repeat(65)]) expect(idError(id)).toBeDefined();
  });
});

describe("intError", () => {
  it("checks the range", () => {
    expect(intError("5", 1, 10)).toBeUndefined();
    expect(intError("0", 1, 10)).toBe("Between 1 and 10");
    expect(intError("5000000000", 1, 4294967295)).toBe("Between 1 and 4294967295");
    expect(intError("4294967295", 1, 4294967295)).toBeUndefined();
  });

  it("wants a whole number", () => {
    for (const v of ["", "1.5", "-1", "1e3"]) expect(intError(v, 0, 10)).toBe("A whole number");
  });
});
