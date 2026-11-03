import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { api, ApiError, setUnauthorizedHandler } from "./api";

type Call = { url: string; init: RequestInit };

function stubFetch(status: number, body: unknown): Call[] {
  const calls: Call[] = [];
  vi.stubGlobal("fetch", async (url: string, init: RequestInit) => {
    calls.push({ url, init });
    const text = body === undefined ? "" : JSON.stringify(body);
    return new Response(text, { status, headers: { "Content-Type": "application/json" } });
  });
  return calls;
}

describe("api client", () => {
  const unauthorized = vi.fn();

  beforeEach(() => setUnauthorizedHandler(unauthorized));
  afterEach(() => {
    setUnauthorizedHandler(null);
    unauthorized.mockReset();
    vi.unstubAllGlobals();
  });

  it("sends JSON bodies and decodes JSON answers", async () => {
    const calls = stubFetch(200, { node: { id: "office-1" }, key: "wfx1.abc" });
    const res = await api.createNode({ id: "office-1", name: "Office" });
    expect(res.key).toBe("wfx1.abc");
    expect(calls[0].url).toBe("/api/nodes");
    expect(calls[0].init.method).toBe("POST");
    expect(calls[0].init.body).toBe(JSON.stringify({ id: "office-1", name: "Office" }));
    expect(calls[0].init.headers).toEqual({ "Content-Type": "application/json" });
  });

  it("escapes path segments and drops empty query parameters", async () => {
    const calls = stubFetch(200, []);
    await api.connections({ user: "a b", node: "" });
    await api.updateUser("x/y", { enabled: false });
    expect(calls[0].url).toBe("/api/connections?user=a+b");
    expect(calls[1].url).toBe("/api/users/x%2Fy");
    expect(calls[1].init.method).toBe("PATCH");
  });

  it("reports the server's error message", async () => {
    stubFetch(409, { error: "node agent is not connected" });
    const err = await api.restartNode("office-1").catch((e: unknown) => e);
    expect(err).toBeInstanceOf(ApiError);
    expect((err as ApiError).status).toBe(409);
    expect((err as ApiError).message).toBe("node agent is not connected");
    expect(unauthorized).not.toHaveBeenCalled();
  });

  it("falls back to the status when the body is not JSON", async () => {
    vi.stubGlobal("fetch", async () => new Response("gateway down", { status: 502, statusText: "Bad Gateway" }));
    await expect(api.overview()).rejects.toThrow("502 Bad Gateway");
  });

  it("signals an expired session", async () => {
    stubFetch(401, { error: "not signed in" });
    await expect(api.nodes()).rejects.toThrow("not signed in");
    expect(unauthorized).toHaveBeenCalledOnce();
  });

  it("does not treat a failed login as an expired session", async () => {
    stubFetch(401, { error: "wrong username or password" });
    await expect(api.login("admin", "nope")).rejects.toBeInstanceOf(ApiError);
    expect(unauthorized).not.toHaveBeenCalled();
  });

  it("turns network failures into an ApiError", async () => {
    vi.stubGlobal("fetch", async () => {
      throw new TypeError("Failed to fetch");
    });
    const err = await api.me().catch((e: unknown) => e);
    expect(err).toBeInstanceOf(ApiError);
    expect((err as ApiError).status).toBe(0);
  });
});
