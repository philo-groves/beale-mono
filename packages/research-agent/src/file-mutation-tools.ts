import { createHash } from "node:crypto";
import { mkdir, open, realpath, writeFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { nowIso } from "./ids.js";
import type { ResearchExecutableTool } from "./tool-registry.js";

const MAX_FILE_BYTES = 1_048_576;
const digest = (content: Uint8Array) => createHash("sha256").update(content).digest("hex");

export function createFileMutationTools(options: {
  workspaceRoot: string;
  protectedPaths?: readonly string[];
}): ResearchExecutableTool[] {
  return (["write", "edit"] as const).map((operation) => {
    const parameters = {
      type: "object", additionalProperties: false,
      required: operation === "write" ? ["path", "content"] : ["path", "oldText", "newText"],
      properties: {
        path: { type: "string", minLength: 1 },
        maxBytes: { type: "integer", minimum: 1, maximum: MAX_FILE_BYTES, description: "Maximum output size in bytes; host budgets can lower this limit." },
        ...(operation === "write" ? { content: { type: "string", maxLength: MAX_FILE_BYTES } } : {
          oldText: { type: "string", minLength: 1, maxLength: MAX_FILE_BYTES },
          newText: { type: "string", maxLength: MAX_FILE_BYTES },
        }),
        expectedHash: { type: "string", pattern: "^[a-f0-9]{64}$", description: "SHA-256 of the existing bytes; required when file.write replaces a file." },
      },
    };
    return {
      descriptor: {
        name: `file.${operation}`, transportName: `file_${operation}`,
        description: operation === "write"
          ? "Write a UTF-8 candidate file. Replacing an existing file requires its expectedHash; relative paths resolve from the workspace. Runs with host privileges."
          : "Edit exactly one literal occurrence in a UTF-8 candidate file. Ambiguous or missing matches fail without changes; relative paths resolve from the workspace. Runs with host privileges.",
        actionClasses: ["synthesize"], sideEffects: "write", requiredPermissions: ["filesystem:write"], inputSchema: parameters,
      },
      parameters: parameters as NonNullable<ResearchExecutableTool["parameters"]>,
      async execute(action) {
        const startedAt = nowIso();
        try {
          const input = action.input;
          if (typeof input.path !== "string" || !input.path.trim()) throw new Error("path must be a nonempty string.");
          const path = resolve(options.workspaceRoot, input.path);
          const canonical = await realpath(path).catch((error: NodeJS.ErrnoException) => {
            if (error.code !== "ENOENT") throw error;
            return path;
          });
          for (const protectedPath of options.protectedPaths ?? []) {
            const protectedCanonical = await realpath(protectedPath).catch(() => resolve(protectedPath));
            const normalize = (value: string) => process.platform === "win32" ? value.toLowerCase() : value;
            if (normalize(canonical) === normalize(protectedCanonical)) throw new Error("This file is host-managed and cannot be changed through file tools.");
          }
          const existing = await readBoundedFile(path).catch((error: NodeJS.ErrnoException) => {
            if (error.code !== "ENOENT") throw error;
            return null;
          });
          if (existing && existing.length > MAX_FILE_BYTES) throw new Error("File exceeds the bounded editing limit.");
          if (input.expectedHash !== undefined && (!existing || input.expectedHash !== digest(existing))) throw new Error("File changed or expectedHash does not match.");
          let content: string;
          if (operation === "write") {
            if (typeof input.content !== "string") throw new Error("content must be a string.");
            if (existing && input.expectedHash === undefined) throw new Error("Replacing an existing file requires expectedHash.");
            content = input.content;
          } else {
            if (!existing) throw new Error("File does not exist.");
            if (existing.includes(0)) throw new Error("Binary files cannot be edited as text.");
            const text = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true }).decode(existing);
            if (typeof input.oldText !== "string" || !input.oldText || typeof input.newText !== "string") throw new Error("oldText must be nonempty and newText must be a string.");
            const offset = text.indexOf(input.oldText);
            if (offset < 0 || text.indexOf(input.oldText, offset + 1) !== -1) throw new Error("oldText must match exactly once.");
            content = text.slice(0, offset) + input.newText + text.slice(offset + input.oldText.length);
          }
          const bytes = Buffer.from(content, "utf8");
          const maxBytes = typeof input.maxBytes === "number" ? Math.min(input.maxBytes, MAX_FILE_BYTES) : MAX_FILE_BYTES;
          if (bytes.length > maxBytes) throw new Error("Content exceeds the bounded writing limit.");
          await mkdir(dirname(path), { recursive: true });
          await writeFile(path, bytes, { flag: existing ? "w" : "wx" });
          return { action, status: "complete", startedAt, completedAt: nowIso(), summary: `File ${operation} completed.`, output: { path, bytesWritten: bytes.length, contentHash: digest(bytes), candidate: true }, followUpActions: [] };
        } catch (error) {
          return { action, status: "error", startedAt, completedAt: nowIso(), summary: `File ${operation} failed.`, error: { message: error instanceof Error ? error.message : String(error) }, followUpActions: [] };
        }
      },
    } satisfies ResearchExecutableTool;
  });
}

async function readBoundedFile(path: string): Promise<Buffer> {
  const handle = await open(path, "r");
  try {
    if ((await handle.stat()).size > MAX_FILE_BYTES) throw new Error("File exceeds the bounded editing limit.");
    const buffer = Buffer.alloc(MAX_FILE_BYTES + 1);
    let offset = 0;
    while (offset < buffer.length) {
      const { bytesRead } = await handle.read(buffer, offset, buffer.length - offset, null);
      if (bytesRead === 0) break;
      offset += bytesRead;
    }
    if (offset > MAX_FILE_BYTES) throw new Error("File exceeds the bounded editing limit.");
    return buffer.subarray(0, offset);
  } finally {
    await handle.close();
  }
}
