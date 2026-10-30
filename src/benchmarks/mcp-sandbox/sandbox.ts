import type { Effect } from "effect/Effect";
import { flatMap, gen, mapError } from "effect/Effect";
import { HttpClient, HttpClientRequest } from "effect/http";
import * as Result from "effect/Result";

import type { ToolDefinition } from "../../harness/core";
import { SolverError } from "../../harness/core";
import { definedValues } from "../../internal/guards";
import { parseSchema, z } from "../../internal/zod";

export const McpToolSchema = z.object({
  name: z.string(),
  description: z.string().optional(),
  inputSchema: z.record(z.string(), z.unknown()).optional(),
});

export type McpTool = z.infer<typeof McpToolSchema>;

export const McpToolListSchema = z.array(McpToolSchema);

export const McpContentBlockSchema = z.object({
  type: z.string(),
  text: z.string().optional(),
});

export type McpContentBlock = z.infer<typeof McpContentBlockSchema>;

export const McpContentBlockListSchema = z.array(McpContentBlockSchema);

export interface McpToolResult {
  readonly text: string;
  readonly isError: boolean;
}

export interface McpSandbox {
  readonly listTools: Effect<readonly McpTool[], SolverError>;
  readonly callTool: (
    name: string,
    args: Readonly<Record<string, unknown>>
  ) => Effect<McpToolResult, SolverError>;
}

export interface McpSandboxConfig {
  readonly label: string;
  readonly url: string;
  readonly token: string;
}

const EMPTY_OBJECT_SCHEMA = { type: "object", properties: {} } as const;

const AUTH_FAILURE_STATUSES = new Set([401, 403]);

export function toToolDefinition(tool: McpTool): ToolDefinition {
  return {
    type: "function",
    function: {
      name: tool.name,
      parameters: tool.inputSchema ?? EMPTY_OBJECT_SCHEMA,
      ...definedValues({ description: tool.description }),
    },
  };
}

export function contentBlocksToText(
  blocks: readonly McpContentBlock[]
): string {
  return blocks
    .map((block) => block.text ?? `[${block.type} content omitted]`)
    .join("\n");
}

export function makeMcpSandbox(
  client: HttpClient.HttpClient,
  config: McpSandboxConfig
): McpSandbox {
  const baseUrl = config.url.replace(/\/+$/, "");
  const readError =
    (path: string) =>
    (error: { message: string }): SolverError =>
      new SolverError({
        message: `${config.label} ${path} response could not be read: ${error.message}`,
      });
  const post = (path: string, body: Readonly<Record<string, unknown>>) =>
    client
      .execute(
        HttpClientRequest.post(`${baseUrl}${path}`).pipe(
          HttpClientRequest.setHeaders({
            Authorization: `Bearer ${config.token}`,
            "Content-Type": "application/json",
          }),
          HttpClientRequest.bodyJsonUnsafe(body)
        )
      )
      .pipe(
        mapError(
          (error) =>
            new SolverError({
              message: `${config.label} ${path} request failed: ${error.message}`,
            })
        )
      );

  const listTools = post("/list-tools", {}).pipe(
    flatMap((response) =>
      gen(function* () {
        if (response.status < 200 || response.status >= 300) {
          const text = yield* response.text.pipe(
            mapError(readError("/list-tools"))
          );
          return yield* new SolverError({
            message: `${config.label} /list-tools HTTP ${response.status}: ${text}`,
          });
        }
        const body: unknown = yield* response.json.pipe(
          mapError(readError("/list-tools"))
        );
        const parsed = parseSchema(McpToolListSchema, body);
        if (Result.isFailure(parsed)) {
          return yield* new SolverError({
            message: `${config.label} /list-tools returned an unexpected body: ${parsed.failure.message}`,
          });
        }
        return parsed.success;
      })
    )
  );

  const callTool = (
    name: string,
    args: Readonly<Record<string, unknown>>
  ): Effect<McpToolResult, SolverError> =>
    post("/call-tool", { tool_name: name, tool_args: args }).pipe(
      flatMap((response) =>
        gen(function* () {
          if (response.status < 200 || response.status >= 300) {
            const text = yield* response.text.pipe(
              mapError(readError("/call-tool"))
            );
            if (AUTH_FAILURE_STATUSES.has(response.status)) {
              return yield* new SolverError({
                message: `${config.label} /call-tool HTTP ${response.status}: ${text}`,
              });
            }
            const firstLine = text.split("\n")[0] ?? "";
            return {
              text: `Error: tool ${name} failed with HTTP ${response.status}: ${firstLine}`,
              isError: true,
            };
          }
          const body: unknown = yield* response.json.pipe(
            mapError(readError("/call-tool"))
          );
          const parsed = parseSchema(McpContentBlockListSchema, body);
          if (Result.isFailure(parsed)) {
            return yield* new SolverError({
              message: `${config.label} /call-tool returned an unexpected body: ${parsed.failure.message}`,
            });
          }
          return { text: contentBlocksToText(parsed.success), isError: false };
        })
      )
    );

  return { listTools, callTool };
}
