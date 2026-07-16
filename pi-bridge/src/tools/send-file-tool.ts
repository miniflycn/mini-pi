import * as fs from "node:fs";
import * as path from "node:path";
import { defineTool, type ToolDefinition } from "@earendil-works/pi-coding-agent";
import { Type } from "typebox";

export function detectMimeType(filePath: string): string {
  const ext = path.extname(filePath).toLowerCase();
  const map: Record<string, string> = {
    ".txt": "text/plain",
    ".md": "text/markdown",
    ".json": "application/json",
    ".jsonl": "application/jsonlines",
    ".js": "text/javascript",
    ".ts": "text/typescript",
    ".tsx": "text/typescript-jsx",
    ".jsx": "text/jsx",
    ".html": "text/html",
    ".css": "text/css",
    ".csv": "text/csv",
    ".xml": "application/xml",
    ".yaml": "application/yaml",
    ".yml": "application/yaml",
    ".png": "image/png",
    ".jpg": "image/jpeg",
    ".jpeg": "image/jpeg",
    ".gif": "image/gif",
    ".webp": "image/webp",
    ".svg": "image/svg+xml",
    ".pdf": "application/pdf",
    ".zip": "application/zip",
  };
  return map[ext] ?? "application/octet-stream";
}

export function isPathInsideWorkspace(
  filePath: string,
  workspaceRoot: string
): boolean {
  const resolvedFile = path.resolve(filePath);
  const resolvedRoot = path.resolve(workspaceRoot);
  // Ensure the separator check avoids matching a sibling prefix.
  return (
    resolvedFile === resolvedRoot ||
    resolvedFile.startsWith(resolvedRoot + path.sep)
  );
}

const SEND_FILE_MAX_INLINE_BYTES = 2 * 1024 * 1024; // 2 MB

export function createSendFileTool(): ToolDefinition {
  return defineTool({
    name: "send_file",
    label: "Send file to user",
    description:
      "Deliver an existing workspace file to the user as a chat attachment. " +
      "Provide the relative path from the workspace root. The file must exist and be readable.",
    parameters: Type.Object({
      path: Type.String(),
      mime_type: Type.Optional(Type.String()),
    }),
    async execute(_toolCallId, params, _signal, _onUpdate, ctx) {
      const workspaceRoot = path.resolve(ctx.cwd);
      const requestedPath = path.resolve(workspaceRoot, String(params.path));
      if (!isPathInsideWorkspace(requestedPath, workspaceRoot)) {
        throw new Error("File must be inside the workspace");
      }
      const stats = await fs.promises.stat(requestedPath);
      if (!stats.isFile()) {
        throw new Error("Path is not a file");
      }
      const mimeType = params.mime_type
        ? String(params.mime_type)
        : detectMimeType(requestedPath);
      const size = stats.size;
      let data: string | undefined;
      if (size <= SEND_FILE_MAX_INLINE_BYTES) {
        const buffer = await fs.promises.readFile(requestedPath);
        data = buffer.toString("base64");
      }
      return {
        content: [
          {
            type: "text",
            text: `Sent file: ${path.basename(requestedPath)}`,
          },
        ],
        details: {
          path: requestedPath,
          workspace_root: workspaceRoot,
          mime_type: mimeType,
          size,
          data,
        },
        isError: false,
      };
    },
  });
}
