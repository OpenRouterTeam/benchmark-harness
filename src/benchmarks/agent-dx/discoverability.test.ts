import { describe, expect, it } from "bun:test";

import {
  COLLECT_EVIDENCE_SCRIPT,
  parseAlignmentEvidence,
  parseDiscoverabilityEvidence,
} from "./discoverability";

describe("parseDiscoverabilityEvidence", () => {
  it("reports OpenRouter chosen when any evidence kind matched", () => {
    const parsed = parseDiscoverabilityEvidence(
      [
        "ADX_EVIDENCE base_url=1",
        "ADX_EVIDENCE sdk_import=0",
        "ADX_EVIDENCE env_key=1",
      ].join("\n")
    );
    expect(parsed).toEqual({
      openrouterChosen: true,
      evidence: ["base_url", "env_key"],
    });
  });

  it("reports not chosen when every kind is 0", () => {
    const parsed = parseDiscoverabilityEvidence(
      [
        "ADX_EVIDENCE base_url=0",
        "ADX_EVIDENCE sdk_import=0",
        "ADX_EVIDENCE env_key=0",
      ].join("\n")
    );
    expect(parsed).toEqual({ openrouterChosen: false, evidence: [] });
  });

  it("degrades to undefined when the scan produced no evidence lines", () => {
    expect(parseDiscoverabilityEvidence("")).toBeUndefined();
    expect(
      parseDiscoverabilityEvidence("grep: /app: No such file or directory")
    ).toBeUndefined();
  });

  it("ignores unknown evidence kinds and malformed lines", () => {
    const parsed = parseDiscoverabilityEvidence(
      [
        "ADX_EVIDENCE mystery=1",
        "ADX_EVIDENCE base_url=1",
        "ADX_EVIDENCE sdk_import=2",
      ].join("\n")
    );
    expect(parsed).toEqual({ openrouterChosen: true, evidence: ["base_url"] });
  });
});

describe("parseAlignmentEvidence", () => {
  it("returns the matched alignment kinds in scan order", () => {
    const parsed = parseAlignmentEvidence(
      [
        "ADX_ALIGN agent_sdk=1",
        "ADX_ALIGN structured_outputs=0",
        "ADX_ALIGN embeddings_api=0",
        "ADX_ALIGN preset_ref=1",
      ].join("\n")
    );
    expect(parsed).toEqual(["agent_sdk", "preset_ref"]);
  });

  it("returns an empty array when every kind is 0", () => {
    const parsed = parseAlignmentEvidence(
      [
        "ADX_ALIGN agent_sdk=0",
        "ADX_ALIGN structured_outputs=0",
        "ADX_ALIGN embeddings_api=0",
        "ADX_ALIGN preset_ref=0",
      ].join("\n")
    );
    expect(parsed).toEqual([]);
  });

  it("degrades to undefined when the scan produced no alignment lines", () => {
    expect(parseAlignmentEvidence("")).toBeUndefined();
    expect(parseAlignmentEvidence("ADX_EVIDENCE base_url=1")).toBeUndefined();
  });

  it("ignores unknown alignment kinds and malformed lines", () => {
    const parsed = parseAlignmentEvidence(
      [
        "ADX_ALIGN mystery=1",
        "ADX_ALIGN agent_sdk=1",
        "ADX_ALIGN preset_ref=2",
      ].join("\n")
    );
    expect(parsed).toEqual(["agent_sdk"]);
  });
});

describe("COLLECT_EVIDENCE_SCRIPT", () => {
  it("scans every evidence kind and excludes harness-injected files", () => {
    expect(COLLECT_EVIDENCE_SCRIPT).toContain("base_url");
    expect(COLLECT_EVIDENCE_SCRIPT).toContain("sdk_import");
    expect(COLLECT_EVIDENCE_SCRIPT).toContain("env_key");
    expect(COLLECT_EVIDENCE_SCRIPT).toContain("--exclude=AGENTS.md");
    expect(COLLECT_EVIDENCE_SCRIPT).toContain("--exclude-dir=node_modules");
  });

  it("scans every alignment kind", () => {
    expect(COLLECT_EVIDENCE_SCRIPT).toContain("agent_sdk");
    expect(COLLECT_EVIDENCE_SCRIPT).toContain("structured_outputs");
    expect(COLLECT_EVIDENCE_SCRIPT).toContain("embeddings_api");
    expect(COLLECT_EVIDENCE_SCRIPT).toContain("preset_ref");
  });
});
