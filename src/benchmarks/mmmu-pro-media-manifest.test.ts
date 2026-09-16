import { describe, expect, it } from "bun:test";

import {
  buildMmmuProMediaManifest,
  hashMmmuProMedia,
} from "./mmmu-pro-media-manifest";
import { mmmuProVisionRecordToSample } from "./mmmu-pro-vision";

const revision = "a".repeat(40);
const sourcePath = `/cached-assets/MMMU/MMMU_Pro/--/${revision}/--/vision/test/0/image/image.png`;
const image = {
  id: "test_History_1",
  sourcePath,
  url: `https://mirror.example/mmmu-pro/${revision}/image.png`,
  bytes: 10,
  contentType: "image/png",
  sha256: "b".repeat(64),
};

function rawManifest(images = [image]) {
  return {
    dataset: "MMMU/MMMU_Pro",
    config: "vision",
    split: "test",
    revision,
    images,
    manifestHash: hashMmmuProMedia(images),
  };
}

describe("MMMU Pro mirrored media", () => {
  it("replaces an expired signed URL without changing the prompt, answer, or image detail", () => {
    const manifest = buildMmmuProMediaManifest(rawManifest());
    const record = {
      id: image.id,
      image: {
        src: `https://datasets-server.huggingface.co${sourcePath}?Expires=1&Signature=expired`,
      },
      options: "['first', 'second']",
      answer: "B",
    };
    const original = mmmuProVisionRecordToSample(record, 0, "low");
    const mirrored = mmmuProVisionRecordToSample(record, 0, "low", manifest);
    expect(mirrored.input).toBe(original.input);
    expect(mirrored.target).toEqual(original.target);
    expect(mirrored.contentParts?.at(1)).toEqual({
      type: "image_url",
      imageUrl: { url: image.url, detail: "low" },
    });
    expect(mirrored.metadata?.["media_manifest_hash"]).toBe(
      manifest.manifestHash
    );
    expect(mirrored.metadata?.["dataset_revision"]).toBe(revision);
    expect(JSON.stringify(mirrored)).not.toContain("Signature");
    expect(JSON.stringify(mirrored)).not.toContain("base64");
    expect(() =>
      mmmuProVisionRecordToSample(
        { ...record, id: "missing" },
        0,
        undefined,
        manifest
      )
    ).toThrow("missing from media manifest");
    expect(() =>
      mmmuProVisionRecordToSample(
        { ...record, image: null },
        0,
        undefined,
        manifest
      )
    ).toThrow("missing or invalid");
    expect(() =>
      mmmuProVisionRecordToSample(
        {
          ...record,
          image: { src: record.image.src.replace(revision, "c".repeat(40)) },
        },
        0,
        undefined,
        manifest
      )
    ).toThrow("changed");
  });

  it("rejects corrupted, duplicate, or wrong-revision manifest entries", () => {
    expect(() =>
      buildMmmuProMediaManifest({
        ...rawManifest(),
        manifestHash: "0".repeat(64),
      })
    ).toThrow("hash");
    expect(() =>
      buildMmmuProMediaManifest(rawManifest([image, image]))
    ).toThrow("duplicate");
    expect(() =>
      buildMmmuProMediaManifest(
        rawManifest([
          {
            ...image,
            sourcePath: sourcePath.replace(revision, "c".repeat(40)),
          },
        ])
      )
    ).toThrow("revision");
    expect(() => buildMmmuProMediaManifest({})).toThrow();
  });
});
