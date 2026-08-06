import { describe, expect, it } from "bun:test";

import {
  getSearchBenchmarkProvenance,
  searchBenchmarkProvenanceIds,
} from "@openrouter/bench-harness/benchmarks/search/provenance";

describe("search benchmark provenance", () => {
  it("exposes pinned BrowseComp provenance without raw prompts", () => {
    expect(getSearchBenchmarkProvenance("search_browsecomp")).toEqual({
      benchmarkId: "search_browsecomp",
      dataset: {
        source:
          "https://openaipublic.blob.core.windows.net/simple-evals/browse_comp_test_set.csv",
        revision: null,
        sha256:
          "7b24471cd5b3eb2a46830a14802b5c029ea62f488ff75a0f88af7923d1454abf",
        rowCount: 1266,
      },
      generationPrompt: {
        sha256:
          "2250aa21b4a98647dbcfed62a30261eb914016c81cb33e7694c15859205de78f",
      },
      judges: [
        {
          model: "openai/gpt-4.1",
          prompt: {
            id: "answer_equivalence_judge",
            sha256:
              "0f0023ee579b8c134f1834ed8952778b9e01460e31d47c242ee3629da9d44835",
          },
        },
      ],
      gradingReferenceDate: null,
    });
  });

  it("exposes pinned DSQA provenance", () => {
    expect(getSearchBenchmarkProvenance("search_dsqa")).toEqual({
      benchmarkId: "search_dsqa",
      dataset: {
        source:
          "https://huggingface.co/datasets/google/deepsearchqa/resolve/b2623f8653065c2672de6d941fc5434cd652376c/DSQA-full.csv",
        revision: "b2623f8653065c2672de6d941fc5434cd652376c",
        sha256:
          "25d48dcf7efa872e5467032e8b8eedf38d301f59a252d0da95cda584baa78396",
        rowCount: 900,
      },
      generationPrompt: {
        sha256:
          "2250aa21b4a98647dbcfed62a30261eb914016c81cb33e7694c15859205de78f",
      },
      judges: [
        {
          model: "google/gemini-2.5-flash",
          prompt: {
            id: "dsqa_judge",
            sha256:
              "7b9279e2d8dba2547eb6e1c00dcd7498150df3fdd3cace6b8e817ea67ae07f9c",
          },
        },
      ],
      gradingReferenceDate: null,
    });
  });

  it("exposes both WideSearch judge prompts and its grading date", () => {
    expect(getSearchBenchmarkProvenance("search_widesearch")).toEqual({
      benchmarkId: "search_widesearch",
      dataset: {
        source:
          "https://huggingface.co/datasets/ByteDance-Seed/WideSearch/resolve/6531a7e5b497d44c8912407e0cb3dc95bd98cc09/widesearch.jsonl",
        revision: "6531a7e5b497d44c8912407e0cb3dc95bd98cc09",
        sha256:
          "bba28ec51dce28fa617f82617d88fcd6bdd4cd4d7f0a4d70db07d7fa8a90bdf4",
        rowCount: 200,
      },
      generationPrompt: {
        sha256:
          "14061a8a9476ecd7f5b5a4ca3de70248bd9a46f6ff963992a813538382ef23e8",
      },
      judges: [
        {
          model: "openai/gpt-4.1",
          prompt: {
            id: "widesearch_alignment",
            sha256:
              "5e2a997c046b43b300cfcca9b3576821d0c46341f65c8f1648e11bccc86b7e84",
          },
        },
        {
          model: "openai/gpt-4.1",
          prompt: {
            id: "widesearch_cell_judge",
            sha256:
              "75d44852d07dff3924c269a4d2838c6a5a85820d8a8b709d65b293fc1525b2a2",
          },
        },
      ],
      gradingReferenceDate: "2026-07-18",
    });
  });

  it("lists only the supported search benchmarks and rejects unknown ids", () => {
    expect(searchBenchmarkProvenanceIds()).toEqual([
      "search_browsecomp",
      "search_dsqa",
      "search_widesearch",
    ]);
    expect(getSearchBenchmarkProvenance("search_hle")).toBeUndefined();
  });
});
