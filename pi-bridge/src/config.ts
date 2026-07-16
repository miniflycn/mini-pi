import { getAgentDir } from "@earendil-works/pi-coding-agent";
import type { BridgeConfig, Logger } from "./types.js";

export function parseArgs(argv: string[]): BridgeConfig {
  let port = 0;
  let agentDir = getAgentDir();

  for (let i = 0; i < argv.length; i++) {
    switch (argv[i]) {
      case "--port": {
        const next = argv[++i];
        port = next ? parseInt(next, 10) || 0 : 0;
        break;
      }
      case "--agent-dir": {
        const next = argv[++i];
        if (next) agentDir = next;
        break;
      }
    }
  }

  return { port, agentDir };
}

export function createConsoleLogger(prefix = "[pi-bridge]"): Logger {
  return {
    debug: (...args: unknown[]) => {
      if (process.env.PI_BRIDGE_DEBUG) {
        console.error(prefix, "[debug]", ...args);
      }
    },
    info: (...args: unknown[]) => console.error(prefix, ...args),
    warn: (...args: unknown[]) => console.error(prefix, "[warn]", ...args),
    error: (...args: unknown[]) => console.error(prefix, "[error]", ...args),
  };
}

export const defaultLogger = createConsoleLogger();
