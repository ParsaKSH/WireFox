import { describe, expect, it } from "vitest";
import { rateTicks, timeTicks } from "./TrafficChart";

describe("rateTicks", () => {
  it("starts at zero and covers the peak with round steps", () => {
    const ticks = rateTicks(900 * 1024);
    expect(ticks[0]).toBe(0);
    expect(ticks[ticks.length - 1]).toBeGreaterThanOrEqual(900 * 1024);
    const step = ticks[1] - ticks[0];
    expect(step).toBe(250 * 1024);
    ticks.forEach((t, i) => expect(t).toBeCloseTo(i * step));
  });

  it("handles an idle chart", () => {
    expect(rateTicks(0)).toEqual([0]);
  });
});

describe("timeTicks", () => {
  const t0 = 1_700_000_003;

  const steps = (ticks: number[]) => new Set(ticks.slice(1).map((t, i) => t - ticks[i]));

  it("shows seconds when the range is shorter than a few minutes", () => {
    const { ticks, seconds } = timeTicks(t0, t0 + 60, 8);
    expect(seconds).toBe(true);
    expect(steps(ticks)).toEqual(new Set([10]));
    ticks.forEach((t) => expect(t % 10).toBe(0));
  });

  it("uses whole minutes for an hour", () => {
    const { ticks, seconds } = timeTicks(t0, t0 + 3600, 8);
    expect(seconds).toBe(false);
    expect(steps(ticks)).toEqual(new Set([600]));
    expect(ticks.length).toBeLessThanOrEqual(8);
  });

  it("stays within the range and the tick budget", () => {
    for (const span of [1, 7, 45, 200, 900, 5000, 86400]) {
      for (const max of [2, 5, 11]) {
        const { ticks } = timeTicks(t0, t0 + span, max);
        expect(ticks.length).toBeGreaterThan(0);
        expect(ticks.length).toBeLessThanOrEqual(max);
        ticks.forEach((t) => {
          expect(t).toBeGreaterThanOrEqual(t0);
          expect(t).toBeLessThanOrEqual(t0 + span);
        });
      }
    }
  });
});
