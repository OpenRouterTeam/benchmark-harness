import { describe, expect, it } from "bun:test";

import {
  LIVE_MCP_BENCH_TASK_METADATA_KEY,
  parseNumberedList,
  recordToSample,
  sampleTask,
} from "./dataset";

const record = {
  task_id: "0e3287cb",
  Question: "Generate a PDF report in /root/pdf.",
  answers: "file /root/pdf/report.pdf",
  category: "Office",
  file_name: "",
  "Annotator Metadata": {
    "Number of steps": "2",
    "Number of tools": "2",
    Steps: "1. Get trends.\n2. Write the PDF.",
    Tools: "1. get-weread-rank\n2. convert_to_pdf",
  },
};

describe("parseNumberedList", () => {
  it("strips the numbering and blank lines", () => {
    expect(parseNumberedList("1. a\n2.  b\n\n3.c\n")).toEqual(["a", "b", "c"]);
  });

  it("keeps unnumbered lines as items", () => {
    expect(parseNumberedList("search\nfetch")).toEqual(["search", "fetch"]);
  });
});

describe("recordToSample", () => {
  it("maps a dataset row to a sample with the task in metadata", () => {
    const sample = recordToSample(record, 3);
    expect(sample.id).toBe("live_mcp_bench-3");
    expect(sample.input).toBe(record.Question);
    expect(sample.target).toEqual({ text: record.answers });
    expect(sampleTask(sample)).toEqual({
      taskId: "0e3287cb",
      category: "Office",
      answers: "file /root/pdf/report.pdf",
      fileName: "",
      steps: "1. Get trends.\n2. Write the PDF.",
      annotatedTools: ["get-weread-rank", "convert_to_pdf"],
    });
  });

  it("rejects a row with an unknown category", () => {
    expect(() => recordToSample({ ...record, category: "Gaming" }, 0)).toThrow(
      /malformed/
    );
  });

  it("returns undefined for a sample without task metadata", () => {
    const sample = recordToSample(record, 0);
    expect(
      sampleTask({
        ...sample,
        metadata: { [LIVE_MCP_BENCH_TASK_METADATA_KEY]: { taskId: 1 } },
      })
    ).toBeUndefined();
  });
});
