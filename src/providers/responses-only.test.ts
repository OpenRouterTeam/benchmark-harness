import { describe, expect, it } from "bun:test";
import { readdir, readFile } from "node:fs/promises";
import { join } from "node:path";

const sourceRoot = join(import.meta.dir, "..");
const forbiddenPath = ["chat", "completions"].join("/");

async function sourceFiles(path: string): Promise<string[]> {
  const entries = await readdir(path, { withFileTypes: true });
  const files: string[] = [];
  for (const entry of entries) {
    const entryPath = join(path, entry.name);
    if (entry.isDirectory()) {
      files.push(...(await sourceFiles(entryPath)));
    } else if (entry.name.endsWith(".ts") && !entry.name.endsWith(".test.ts")) {
      files.push(entryPath);
    }
  }
  return files;
}

describe("Responses-only transport", () => {
  it("does not retain a Chat Completions endpoint in source", async () => {
    const files = await sourceFiles(sourceRoot);
    const matches = [];
    for (const file of files) {
      const contents = await readFile(file, "utf8");
      if (contents.includes(forbiddenPath)) {
        matches.push(file);
      }
    }
    expect(matches).toEqual([]);
  });
});
