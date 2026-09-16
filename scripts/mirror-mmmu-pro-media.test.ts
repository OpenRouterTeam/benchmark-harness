import { afterEach, beforeEach, expect, it, spyOn } from "bun:test";
import { createHash } from "node:crypto";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { S3Client, type S3File } from "bun";

import { mirrorMmmuProMedia } from "./mirror-mmmu-pro-media";

const revision = "a".repeat(40);
const sourcePrefix = `https://datasets-server.huggingface.co/cached-assets/MMMU/MMMU_Pro/--/${revision}/--/vision/test`;
const rows = ["png", "jpg"].map((extension, index) => ({
  row: {
    id: String(index),
    image: {
      src: `${sourcePrefix}/${index}/image/image.${extension}?Signature=private`,
    },
  },
}));
const bytes = new Uint8Array([1, 2, 3]);
const env = {
  BENCH_MEDIA_S3_ENDPOINT: "https://storage.example",
  BENCH_MEDIA_S3_BUCKET: "media",
  BENCH_MEDIA_S3_ACCESS_KEY_ID: "fixture",
  BENCH_MEDIA_S3_SECRET_ACCESS_KEY: "fixture",
  BENCH_MEDIA_PUBLIC_BASE_URL: "https://mirror.example",
  BENCH_MEDIA_KEY_PREFIX: "",
};
const originalEnv = Object.fromEntries(
  Object.keys(env).map((name) => [name, process.env[name]])
);
const originalFetch = globalThis.fetch;
const uploads = new Map<string, { bytes: Uint8Array; contentType: string }>();
let directory: string;
let restoreStorage: () => void;
let sourceRevision: string;
let corruptReadback: boolean;

beforeEach(async () => {
  directory = await mkdtemp(join(tmpdir(), "mmmu-mirror-test-"));
  Object.assign(process.env, env);
  uploads.clear();
  sourceRevision = revision;
  corruptReadback = false;
  const storage = spyOn(S3Client.prototype, "file").mockImplementation(
    (key) =>
      ({
        write: async (data: Uint8Array, options: { type: string }) => {
          uploads.set(`/${key}`, { bytes: data, contentType: options.type });
          return data.length;
        },
      }) as unknown as S3File
  );
  restoreStorage = () => storage.mockRestore();
  globalThis.fetch = async (input) => {
    const url = new URL(String(input));
    if (url.pathname === "/rows") {
      return Response.json(
        { rows, num_rows_total: 2 },
        { headers: { "x-revision": sourceRevision } }
      );
    }
    if (url.origin === "https://datasets-server.huggingface.co") {
      return new Response(bytes);
    }
    const uploaded = uploads.get(url.pathname)!;
    return new Response(
      corruptReadback ? new Uint8Array([9, 9, 9]) : uploaded.bytes,
      {
        headers: { "content-type": uploaded.contentType },
      }
    );
  };
});

afterEach(async () => {
  globalThis.fetch = originalFetch;
  restoreStorage();
  for (const [name, value] of Object.entries(originalEnv)) {
    if (value === undefined) {
      Reflect.deleteProperty(process.env, name);
    } else {
      process.env[name] = value;
    }
  }
  await rm(directory, { recursive: true, force: true });
});

it("publishes original PNG/JPEG bytes and emits the verified manifest", async () => {
  const out = join(directory, "manifest.json");
  const manifest = await mirrorMmmuProMedia({ revision, out });
  expect(manifest.images.map((image) => image.contentType)).toEqual([
    "image/png",
    "image/jpeg",
  ]);
  expect(uploads.size).toBe(2);
  expect(
    [...uploads.values()].every((upload) =>
      Buffer.from(upload.bytes).equals(bytes)
    )
  ).toBe(true);
  expect(manifest.images[0]?.sha256).toBe(
    createHash("sha256").update(bytes).digest("hex")
  );
  expect(JSON.stringify(manifest)).not.toContain("Signature");
  expect(await Bun.file(out).json()).toEqual(manifest);
});

it("rejects an unexpected HF revision before uploading", async () => {
  sourceRevision = "b".repeat(40);
  const out = join(directory, "manifest.json");
  await expect(mirrorMmmuProMedia({ revision, out })).rejects.toThrow(
    "revision"
  );
  expect(uploads.size).toBe(0);
  expect(await Bun.file(out).exists()).toBe(false);
});

it("does not emit a manifest when a same-size public response has different bytes", async () => {
  corruptReadback = true;
  const out = join(directory, "manifest.json");
  await expect(mirrorMmmuProMedia({ revision, out })).rejects.toThrow(
    "readback verification"
  );
  expect(await Bun.file(out).exists()).toBe(false);
});
