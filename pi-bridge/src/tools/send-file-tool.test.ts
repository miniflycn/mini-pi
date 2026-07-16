import { describe, test, expect } from "bun:test";
import { isPathInsideWorkspace, detectMimeType } from "./send-file-tool.js";

describe("send-file-tool", () => {
  describe("isPathInsideWorkspace", () => {
    test("allows a file inside the workspace", () => {
      expect(isPathInsideWorkspace("/workspace/report.txt", "/workspace")).toBe(
        true
      );
    });

    test("allows a nested file inside the workspace", () => {
      expect(
        isPathInsideWorkspace("/workspace/docs/readme.md", "/workspace")
      ).toBe(true);
    });

    test("rejects a file outside the workspace", () => {
      expect(isPathInsideWorkspace("/etc/passwd", "/workspace")).toBe(false);
    });

    test("rejects a path that is a sibling prefix of the workspace", () => {
      expect(
        isPathInsideWorkspace("/workspace-evil/file.txt", "/workspace")
      ).toBe(false);
    });

    test("rejects traversal outside the workspace", () => {
      expect(
        isPathInsideWorkspace("/workspace/../outside.txt", "/workspace")
      ).toBe(false);
    });
  });

  describe("detectMimeType", () => {
    test("detects common mime types by extension", () => {
      expect(detectMimeType("file.txt")).toBe("text/plain");
      expect(detectMimeType("file.png")).toBe("image/png");
      expect(detectMimeType("file.pdf")).toBe("application/pdf");
    });

    test("falls back to octet-stream for unknown extensions", () => {
      expect(detectMimeType("file.unknown")).toBe("application/octet-stream");
    });
  });
});
