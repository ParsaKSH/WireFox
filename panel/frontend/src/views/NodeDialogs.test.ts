import { describe, expect, it } from "vitest";
import type { Node, NodePatch } from "../types";
import { changedFields } from "./NodeDialogs";

const node = {
  id: "office-1",
  name: "Office",
  enabled: true,
  config: {
    version: 3,
    pool_size: 8,
    max_extra: 64,
    connect_timeout_secs: 10,
    allow_local: false,
    egress: { interface: "eth0", source_ip: "" },
    uplink: { interface: "", source_ip: "" },
  },
} as Node;

const unchanged: Required<NodePatch> = {
  name: "Office",
  enabled: true,
  pool_size: 8,
  max_extra: 64,
  connect_timeout_secs: 10,
  allow_local: false,
  egress: { interface: "eth0", source_ip: "" },
  uplink: { interface: "", source_ip: "" },
};

describe("changedFields", () => {
  it("sends nothing when nothing changed", () => {
    expect(changedFields(node, unchanged)).toEqual({});
  });

  it("leaves name and enabled out of a tunnel-only edit", () => {
    const patch = changedFields(node, { ...unchanged, egress: { interface: "wg0", source_ip: "" }, pool_size: 4 });
    expect(patch).toEqual({ egress: { interface: "wg0", source_ip: "" }, pool_size: 4 });
  });

  it("sends name and enabled when they changed", () => {
    expect(changedFields(node, { ...unchanged, name: "HQ", enabled: false })).toEqual({ name: "HQ", enabled: false });
  });
});
