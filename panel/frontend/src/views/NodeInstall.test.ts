import { describe, expect, it } from "vitest";
import { linuxSteps, openwrtSteps, windowsScript } from "./NodeInstall";

const KEY = "wfx1.example-secret";

// Lines that put the key somewhere only root (or the service) can read it.
const STORES_KEY = [
  /^printf 'WIREFOX_NODE_KEY=%s\\n' '.+' > \/etc\/wirefox\/node\.env$/,
  /^echo '.+' > \/etc\/wirefox\/node\.key$/,
  /^setx \/M WIREFOX_NODE_KEY '.+'$/,
  /^\$env:WIREFOX_NODE_KEY = '.+'$/,
];

function snippets(): [string, string][] {
  const linux = linuxSteps(KEY);
  const openwrt = openwrtSteps(KEY);
  return [
    ["linux shell", linux.shell],
    ["linux unit", linux.unit],
    ["linux enable", linux.enable],
    ["windows", windowsScript(KEY)],
    ["openwrt shell", openwrt.shell],
    ["openwrt init", openwrt.init],
    ["openwrt enable", openwrt.enable],
  ];
}

describe("install snippets", () => {
  it("never put the key on a command line", () => {
    for (const [name, code] of snippets()) {
      expect(code, name).not.toContain("--key");
      for (const line of code.split("\n").filter((l) => l.includes(KEY))) {
        expect(
          STORES_KEY.some((re) => re.test(line)),
          `${name}: ${line}`,
        ).toBe(true);
      }
    }
  });

  it("run the node from the environment on Linux", () => {
    const { unit } = linuxSteps(KEY);
    expect(unit).toContain("EnvironmentFile=/etc/wirefox/node.env");
    expect(unit).toContain("ExecStart=/usr/local/bin/wirefox-node run\n");
    expect(unit).toContain("StateDirectory=wirefox-node");
    expect(linuxSteps(KEY).shell).toContain("install -m 600 /dev/null /etc/wirefox/node.env");
  });

  it("keep OpenWrt state on flash and pass the key through procd", () => {
    const { init } = openwrtSteps(KEY);
    expect(init).toContain("procd_set_param command /usr/bin/wirefox-node run --state-dir /etc/wirefox-node\n");
    expect(init).toContain('procd_set_param env WIREFOX_NODE_KEY="$(cat /etc/wirefox/node.key)"');
  });

  it("start the Windows task with just run", () => {
    expect(windowsScript(KEY)).toContain("-Execute 'C:\\WireFox\\wirefox-node.exe' -Argument 'run'");
  });
});
