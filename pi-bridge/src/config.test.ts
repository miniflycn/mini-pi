import { describe, test, expect } from "bun:test";
import { parseArgs } from "./config.js";

describe("config", () => {
  test("parses --port and --agent-dir", () => {
    const config = parseArgs(["--port", "1234", "--agent-dir", "/tmp/agent"]);
    expect(config.port).toBe(1234);
    expect(config.agentDir).toBe("/tmp/agent");
  });

  test("defaults port to 0", () => {
    const config = parseArgs([]);
    expect(config.port).toBe(0);
  });

  test("ignores unknown flags", () => {
    const config = parseArgs(["--unknown", "value"]);
    expect(config.port).toBe(0);
  });
});
