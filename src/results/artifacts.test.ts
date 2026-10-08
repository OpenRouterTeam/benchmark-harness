import { describe, expect, it } from "bun:test";
import { createHash } from "node:crypto";

import { either } from "effect/Effect";

import { MessageRole, ScoreValue } from "../harness/core";
import type { SampleScore } from "../harness/metric";
import type { RunResult } from "../harness/run";
import { runHarnessPromise } from "../internal/effect-logger";
import type { ArtifactSink, InlineArtifact } from "./artifacts";
import { ArtifactUploadError, offloadInlineBase64 } from "./artifacts";

const PNG = Buffer.from("png-bytes").toString("base64");
const WAV = Buffer.from("wav-bytes").toString("base64");
const PDF = Buffer.from("pdf-bytes").toString("base64");

function sha256(base64: string): string {
  return createHash("sha256")
    .update(Buffer.from(base64, "base64"))
    .digest("hex");
}

function runResult(samples: readonly Partial<SampleScore>[]): RunResult {
  return {
    metrics: {
      accuracy: 1,
      totalQuestions: samples.length,
      correctAnswers: samples.length,
      skippedQuestions: 0,
    },
    usage: {
      inputTokens: 0,
      outputTokens: 0,
      totalTokens: 0,
      reasoningTokens: 0,
      totalCost: 0,
      generationTimeMs: 0,
    },
    sampleScores: samples.map((sample, index) => ({
      sampleId: `s${index}`,
      epoch: 0,
      score: { value: ScoreValue.Correct, answer: "A", explanation: "" },
      ...sample,
    })),
  };
}

function memorySink(failures = 0): ArtifactSink & {
  readonly puts: InlineArtifact[];
} {
  const puts: InlineArtifact[] = [];
  let remainingFailures = failures;
  return {
    puts,
    uriFor: (path) => `gs://results/run-1/${path}`,
    put: (artifact) => {
      puts.push(artifact);
      if (remainingFailures > 0) {
        remainingFailures -= 1;
        return Promise.reject(new Error("503 from bucket"));
      }
      return Promise.resolve();
    },
  };
}

function offload(result: RunResult, sink: ArtifactSink): Promise<RunResult> {
  return runHarnessPromise(
    offloadInlineBase64(result, sink, { baseDelayMs: 1 })
  );
}

describe("offloadInlineBase64", () => {
  it("replaces an MMMU data URL image with the uploaded pointer", async () => {
    const sink = memorySink();
    const result = await offload(
      runResult([
        {
          messages: [
            {
              role: MessageRole.User,
              content: "Which organ?",
              contentParts: [
                { type: "text", text: "Which organ?" },
                {
                  type: "image_url",
                  imageUrl: { url: `data:image/png;base64,${PNG}` },
                },
              ],
            },
          ],
        },
      ]),
      sink
    );

    const path = `artifacts/sha256/${sha256(PNG)}.png`;
    expect(result.sampleScores[0]?.messages?.[0]?.contentParts).toEqual([
      { type: "text", text: "Which organ?" },
      { type: "image_url", imageUrl: { url: `gs://results/run-1/${path}` } },
    ]);
    expect(sink.puts).toEqual([
      { path, contentType: "image/png", bytes: Buffer.from(PNG, "base64") },
    ]);
  });

  it("replaces the known bare base64 fields with pointers", async () => {
    const sink = memorySink();
    const result = await offload(
      runResult([
        {
          metadata: {
            oriImage: { type: "image", data: PNG, mimeType: "image/jpeg" },
            anthropicImage: {
              type: "image",
              source: { type: "base64", media_type: "image/webp", data: PNG },
            },
          },
          requestBody: {
            messages: [
              {
                role: "user",
                content: [
                  {
                    type: "input_audio",
                    input_audio: { data: WAV, format: "wav" },
                  },
                  { type: "file", file: { filename: "a.pdf", file_data: PDF } },
                  {
                    type: "file",
                    file: { file_data: `data:application/pdf;base64,${PDF}` },
                  },
                ],
              },
            ],
          },
        },
      ]),
      sink
    );

    const uri = (base64: string, ext: string) =>
      `gs://results/run-1/artifacts/sha256/${sha256(base64)}.${ext}`;
    expect(result.sampleScores[0]).toMatchObject({
      metadata: {
        oriImage: {
          type: "image",
          data: uri(PNG, "jpg"),
          mimeType: "image/jpeg",
        },
        anthropicImage: {
          source: {
            type: "base64",
            media_type: "image/webp",
            data: uri(PNG, "webp"),
          },
        },
      },
      requestBody: {
        messages: [
          {
            content: [
              { input_audio: { data: uri(WAV, "wav"), format: "wav" } },
              { file: { filename: "a.pdf", file_data: uri(PDF, "bin") } },
              { file: { file_data: uri(PDF, "pdf") } },
            ],
          },
        ],
      },
    });
    expect(sink.puts.map((artifact) => artifact.contentType).sort()).toEqual([
      "application/octet-stream",
      "application/pdf",
      "audio/wav",
      "image/jpeg",
      "image/webp",
    ]);
  });

  it("keeps a pointer at every repeated Ori image while uploading it once", async () => {
    const sink = memorySink();
    const image = { type: "image", data: PNG, mimeType: "image/png" };
    const toolResult = { role: "toolResult", content: [image] };
    const result = await offload(
      runResult([
        {
          metadata: {
            events: [
              { type: "tool_execution_end", result: { content: [image] } },
              { type: "message_end", message: toolResult },
              { type: "agent_end", messages: [toolResult, toolResult] },
            ],
          },
        },
        {
          metadata: { events: [{ type: "agent_end", messages: [toolResult] }] },
        },
      ]),
      sink
    );

    const pointer = {
      ...image,
      data: `gs://results/run-1/artifacts/sha256/${sha256(PNG)}.png`,
    };
    const pointerResult = { role: "toolResult", content: [pointer] };
    expect(result.sampleScores.map((sample) => sample.metadata)).toEqual([
      {
        events: [
          { type: "tool_execution_end", result: { content: [pointer] } },
          { type: "message_end", message: pointerResult },
          { type: "agent_end", messages: [pointerResult, pointerResult] },
        ],
      },
      { events: [{ type: "agent_end", messages: [pointerResult] }] },
    ]);
    expect(sink.puts).toHaveLength(1);
  });

  it("leaves text, hashes and non-base64 fields untouched", async () => {
    const sink = memorySink();
    const input = runResult([
      {
        input: "data:image/png;base64, is how images are inlined",
        messages: [{ role: MessageRole.Assistant, content: PNG }],
        responseItems: [{ id: "rs_abc123==", type: "reasoning", data: PNG }],
        metadata: {
          hash: sha256(PNG),
          plain: "data:text/plain,hello",
          remote: {
            type: "image_url",
            image_url: { url: "https://x.test/a.png" },
          },
          notBase64: { data: "not base64!", mimeType: "image/png" },
        },
      },
    ]);

    const result = await offload(input, sink);

    expect(result).toEqual(input);
    expect(sink.puts).toHaveLength(0);
  });

  it("retries a failed upload and uses the pointer once it succeeds", async () => {
    const sink = memorySink(2);
    const result = await offload(
      runResult([{ metadata: { url: `data:image/png;base64,${PNG}` } }]),
      sink
    );

    expect(result.sampleScores[0]?.metadata).toEqual({
      url: `gs://results/run-1/artifacts/sha256/${sha256(PNG)}.png`,
    });
    expect(sink.puts).toHaveLength(3);
  });

  it("fails instead of keeping the payload inline when retries run out", async () => {
    const sink = memorySink(Number.POSITIVE_INFINITY);
    const result = await runHarnessPromise(
      either(
        offloadInlineBase64(
          runResult([{ metadata: { url: `data:image/png;base64,${PNG}` } }]),
          sink,
          { maxRetries: 2, baseDelayMs: 1 }
        )
      )
    );

    expect(result._tag).toBe("Left");
    expect(result._tag === "Left" ? result.left : undefined).toBeInstanceOf(
      ArtifactUploadError
    );
    expect(result._tag === "Left" ? result.left.message : "").toContain(
      "503 from bucket"
    );
    expect(sink.puts).toHaveLength(3);
  });
});
