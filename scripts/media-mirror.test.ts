import { afterEach, describe, expect, test } from "bun:test";

import type { S3Client, S3File } from "bun";

import { z } from "../src/internal/zod";
import {
  describeFailure,
  fetchHfRowPages,
  hashManifestEntries,
  normalizeKeyPrefix,
  normalizeS3Endpoint,
  sha256Hex,
  uploadUnlessPresent,
  verifyPublished,
} from "./media-mirror";

const originalFetch = globalThis.fetch;

afterEach(() => {
  globalThis.fetch = originalFetch;
});

function fakeS3(existingSize: number | undefined) {
  const writes: { key: string; bytes: Uint8Array; type: string }[] = [];
  const s3 = {
    file: (key: string) =>
      ({
        stat: async () => {
          if (existingSize === undefined) {
            throw new Error("NoSuchKey");
          }
          return { size: existingSize };
        },
        write: async (bytes: Uint8Array, options: { type: string }) => {
          writes.push({ key, bytes, type: options.type });
          return bytes.byteLength;
        },
      }) as unknown as S3File,
  } as unknown as S3Client;
  return { s3, writes };
}

describe("normalizeKeyPrefix", () => {
  test("appends a single trailing slash to a real prefix", () => {
    expect(normalizeKeyPrefix("vgi-bench")).toBe("vgi-bench/");
    expect(normalizeKeyPrefix("/vgi-bench/v1/")).toBe("vgi-bench/v1/");
  });

  test("treats blank and slash-only prefixes as absent", () => {
    expect(normalizeKeyPrefix(undefined)).toBe("");
    expect(normalizeKeyPrefix("  ")).toBe("");
    expect(normalizeKeyPrefix("///")).toBe("");
  });
});

describe("normalizeS3Endpoint", () => {
  test("strips a bucket-suffixed endpoint as shown in the R2 dashboard", () => {
    expect(
      normalizeS3Endpoint(
        "https://acct.r2.cloudflarestorage.com/mmmu-pro-mirror/",
        "mmmu-pro-mirror"
      )
    ).toBe("https://acct.r2.cloudflarestorage.com");
  });

  test("leaves an account-level endpoint untouched", () => {
    expect(
      normalizeS3Endpoint(
        "https://acct.r2.cloudflarestorage.com",
        "mmmu-pro-mirror"
      )
    ).toBe("https://acct.r2.cloudflarestorage.com");
  });
});

describe("describeFailure", () => {
  test("reports the message of an error", () => {
    expect(describeFailure(new Error("Download failed with 503"))).toBe(
      "Download failed with 503"
    );
  });

  test("stringifies non-error causes", () => {
    expect(describeFailure("socket hang up")).toBe("socket hang up");
  });
});

describe("fetchHfRowPages", () => {
  test("pages through rows, validates each row, and surfaces the resolved revision", async () => {
    const requested: URL[] = [];
    globalThis.fetch = async (input) => {
      const url = new URL(String(input));
      requested.push(url);
      const offset = Number(url.searchParams.get("offset"));
      const rows =
        offset === 0
          ? [{ row: { id: "a" } }, { row: { id: "b" } }]
          : [{ row: { id: "c" } }];
      return Response.json(
        { rows, num_rows_total: 3 },
        { headers: { "x-revision": "deadbeef" } }
      );
    };
    const pages = [];
    for await (const page of fetchHfRowPages(
      {
        dataset: "org/ds",
        config: "default",
        split: "test",
        revision: "v1",
      },
      z.object({ id: z.string() })
    )) {
      pages.push(page);
    }
    expect(pages.map((page) => page.rows.map((row) => row.id))).toEqual([
      ["a", "b"],
      ["c"],
    ]);
    expect(pages.map((page) => page.offset)).toEqual([0, 2]);
    expect(pages[0]?.total).toBe(3);
    expect(pages[0]?.resolvedRevision).toBe("deadbeef");
    expect(requested.map((url) => url.searchParams.get("revision"))).toEqual([
      "v1",
      "v1",
    ]);
  });

  test("rejects rows that do not match the schema", async () => {
    globalThis.fetch = async () =>
      Response.json({ rows: [{ row: { id: 1 } }], num_rows_total: 1 });
    const iterate = async () => {
      for await (const _page of fetchHfRowPages(
        { dataset: "org/ds", config: "default", split: "test", revision: "v1" },
        z.object({ id: z.string() })
      )) {
        continue;
      }
    };
    await expect(iterate()).rejects.toThrow();
  });
});

describe("uploadUnlessPresent", () => {
  const bytes = new Uint8Array([1, 2, 3]);

  test("skips the upload when an object of the same size exists", async () => {
    const { s3, writes } = fakeS3(3);
    const outcome = await uploadUnlessPresent(s3, "k", bytes, "image/png", {
      force: false,
      dryRun: false,
    });
    expect(outcome).toBe("skipped");
    expect(writes).toHaveLength(0);
  });

  test("uploads when the object is missing or has a different size", async () => {
    const missing = fakeS3(undefined);
    expect(
      await uploadUnlessPresent(missing.s3, "k", bytes, "image/png", {
        force: false,
        dryRun: false,
      })
    ).toBe("uploaded");
    expect(missing.writes).toEqual([{ key: "k", bytes, type: "image/png" }]);
    const differing = fakeS3(99);
    expect(
      await uploadUnlessPresent(differing.s3, "k", bytes, "image/png", {
        force: false,
        dryRun: false,
      })
    ).toBe("uploaded");
  });

  test("honours force and dry-run", async () => {
    const forced = fakeS3(3);
    expect(
      await uploadUnlessPresent(forced.s3, "k", bytes, "image/png", {
        force: true,
        dryRun: false,
      })
    ).toBe("uploaded");
    const dry = fakeS3(undefined);
    expect(
      await uploadUnlessPresent(dry.s3, "k", bytes, "image/png", {
        force: false,
        dryRun: true,
      })
    ).toBe("dry-run");
    expect(dry.writes).toHaveLength(0);
  });
});

describe("verifyPublished", () => {
  const bytes = new Uint8Array([1, 2, 3]);
  const expected = { sha256: sha256Hex(bytes), contentType: "image/png" };

  test("accepts a public object with matching bytes and content type", async () => {
    globalThis.fetch = async () =>
      new Response(bytes, {
        headers: { "content-type": "image/png; charset=binary" },
      });
    await expect(
      verifyPublished("https://mirror.example/k", expected)
    ).resolves.toBeUndefined();
  });

  test("rejects a non-2xx response, different bytes, or different content type", async () => {
    globalThis.fetch = async () => new Response(null, { status: 404 });
    await expect(
      verifyPublished("https://mirror.example/k", expected)
    ).rejects.toThrow("HTTP 404");
    globalThis.fetch = async () =>
      new Response(new Uint8Array([9, 9, 9]), {
        headers: { "content-type": "image/png" },
      });
    await expect(
      verifyPublished("https://mirror.example/k", expected)
    ).rejects.toThrow("readback verification");
    globalThis.fetch = async () =>
      new Response(bytes, { headers: { "content-type": "image/jpeg" } });
    await expect(
      verifyPublished("https://mirror.example/k", expected)
    ).rejects.toThrow("readback verification");
  });
});

describe("hashManifestEntries", () => {
  test("is stable for identical entries and changes with content", () => {
    const entry = {
      id: "a",
      url: "https://m.example/a",
      sha256: "1".repeat(64),
    };
    const first = hashManifestEntries([entry]);
    expect(hashManifestEntries([entry])).toBe(first);
    expect(
      hashManifestEntries([{ ...entry, sha256: "2".repeat(64) }])
    ).not.toBe(first);
    expect(
      hashManifestEntries([{ ...entry, url: "https://m.example/b" }])
    ).not.toBe(first);
  });
});
