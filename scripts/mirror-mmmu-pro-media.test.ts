import { afterEach, expect, it } from "bun:test";
import { createHash } from "node:crypto";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { prepareMmmuProMedia } from "./mirror-mmmu-pro-media";

const originalFetch = globalThis.fetch;
let directory: string | undefined;
afterEach(async () => {
  globalThis.fetch = originalFetch;
  if (directory) {
    await rm(directory, { recursive: true, force: true });
  }
});

it("prepares original bytes and a complete manifest without storage credentials", async () => {
  directory = await mkdtemp(join(tmpdir(), "mmmu-mirror-test-"));
  const revision = "a".repeat(40);
  const bytes = new Uint8Array([137, 80, 78, 71]);
  const row = {
    id: "sample",
    image: {
      src: `https://datasets-server.huggingface.co/cached-assets/MMMU/MMMU_Pro/--/${revision}/--/vision/test/0/image/image.png?Signature=private`,
    },
  };
  globalThis.fetch = async (input) =>
    new URL(String(input)).pathname === "/rows"
      ? Response.json(
          { rows: [{ row }], num_rows_total: 1 },
          { headers: { "x-revision": revision } }
        )
      : new Response(bytes);
  const manifest = await prepareMmmuProMedia({
    directory,
    revision,
    publicBaseUrl: "https://mirror.example",
  });
  const sha256 = createHash("sha256").update(bytes).digest("hex");
  expect(manifest.images[0]?.sha256).toBe(sha256);
  expect(manifest.images[0]?.url).toBe(
    `https://mirror.example/mmmu-pro/${revision}/${sha256}.png`
  );
  expect(
    new Uint8Array(
      await Bun.file(join(directory, `${sha256}.png`)).arrayBuffer()
    )
  ).toEqual(bytes);
  expect(JSON.stringify(manifest)).not.toContain("Signature");
  expect(await Bun.file(join(directory, "manifest.json")).json()).toEqual(
    manifest
  );
});

it("rejects an unexpected HF revision before downloading images", async () => {
  directory = await mkdtemp(join(tmpdir(), "mmmu-mirror-test-"));
  globalThis.fetch = async () =>
    Response.json({}, { headers: { "x-revision": "b".repeat(40) } });
  await expect(
    prepareMmmuProMedia({
      directory,
      revision: "a".repeat(40),
      publicBaseUrl: "https://mirror.example",
    })
  ).rejects.toThrow("revision");
  expect(await Bun.file(join(directory, "manifest.json")).exists()).toBe(false);
});
