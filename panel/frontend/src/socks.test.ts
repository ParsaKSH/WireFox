import { describe, expect, it } from "vitest";
import { generatePassword, portOf, socksUrl } from "./socks";

describe("portOf", () => {
  it("takes the port of IPv4 and IPv6 listen addresses", () => {
    expect(portOf("0.0.0.0:1080")).toBe("1080");
    expect(portOf("[::]:1443")).toBe("1443");
  });

  it("returns null without a port", () => {
    expect(portOf(undefined)).toBeNull();
    expect(portOf("")).toBeNull();
    expect(portOf("localhost")).toBeNull();
    expect(portOf("localhost:")).toBeNull();
  });
});

describe("socksUrl", () => {
  it("includes the password when it is known", () => {
    expect(socksUrl("alice", "s3cret", "relay.example.com", "1080")).toBe(
      "socks5://alice:s3cret@relay.example.com:1080",
    );
  });

  it("leaves the password out otherwise", () => {
    expect(socksUrl("alice", null, "relay.example.com", "1080")).toBe("socks5://alice@relay.example.com:1080");
  });

  it("escapes credentials and brackets IPv6 hosts", () => {
    expect(socksUrl("a@b", "p:w/x", "2001:db8::1", "1080")).toBe("socks5://a%40b:p%3Aw%2Fx@[2001:db8::1]:1080");
    expect(socksUrl("bob", null, "[2001:db8::1]", null)).toBe("socks5://bob@[2001:db8::1]");
  });
});

describe("generatePassword", () => {
  it("produces unambiguous characters of the requested length", () => {
    const pw = generatePassword(32);
    expect(pw).toHaveLength(32);
    expect(pw).toMatch(/^[a-km-zA-HJ-NP-Z2-9]+$/);
    expect(generatePassword()).not.toBe(generatePassword());
  });
});
