import { describe, expect, it } from "vitest";
import { arrowTarget } from "./ui";

describe("arrowTarget", () => {
  it("moves through a vertical menu and wraps around", () => {
    expect(arrowTarget("ArrowDown", 0, 3, "vertical")).toBe(1);
    expect(arrowTarget("ArrowDown", 2, 3, "vertical")).toBe(0);
    expect(arrowTarget("ArrowUp", 0, 3, "vertical")).toBe(2);
    expect(arrowTarget("Home", 2, 3, "vertical")).toBe(0);
    expect(arrowTarget("End", 0, 3, "vertical")).toBe(2);
  });

  it("starts from the ends when nothing is focused", () => {
    expect(arrowTarget("ArrowDown", -1, 3, "vertical")).toBe(0);
    expect(arrowTarget("ArrowUp", -1, 3, "vertical")).toBe(2);
  });

  it("uses left and right for tabs", () => {
    expect(arrowTarget("ArrowRight", 0, 3, "horizontal")).toBe(1);
    expect(arrowTarget("ArrowLeft", 0, 3, "horizontal")).toBe(2);
    expect(arrowTarget("ArrowDown", 0, 3, "horizontal")).toBeNull();
  });

  it("ignores other keys and empty lists", () => {
    expect(arrowTarget("Enter", 0, 3, "vertical")).toBeNull();
    expect(arrowTarget("ArrowDown", -1, 0, "vertical")).toBeNull();
  });
});
