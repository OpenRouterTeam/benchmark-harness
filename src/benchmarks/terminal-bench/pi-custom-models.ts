export interface PiRouterModelDef {
  readonly id: string;
  readonly name: string;
  readonly costPerMillion: {
    readonly input: number;
    readonly output: number;
    readonly cacheRead: number;
    readonly cacheWrite: number;
  };
  readonly contextWindow: number;
  readonly maxTokens: number;
}

export const PI_UNKNOWN_ROUTER_MODELS: readonly PiRouterModelDef[] = [
  {
    id: "openrouter/auto",
    name: "OpenRouter: Auto Router",
    costPerMillion: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
    contextWindow: 2000000,
    maxTokens: 128000,
  },
  {
    id: "openrouter/auto-beta",
    name: "OpenRouter: Auto Router (Beta)",
    costPerMillion: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
    contextWindow: 2000000,
    maxTokens: 128000,
  },
  {
    id: "openrouter/phaser",
    name: "OpenRouter: Phaser",
    costPerMillion: { input: 2, output: 10, cacheRead: 0.2, cacheWrite: 2.5 },
    contextWindow: 1000000,
    maxTokens: 128000,
  },
];

const OPENROUTER_PROVIDER = "openrouter" as const;

export function findPiRouterModelDef(
  provider: string,
  modelId: string
): PiRouterModelDef | undefined {
  if (provider !== OPENROUTER_PROVIDER) {
    return undefined;
  }
  return PI_UNKNOWN_ROUTER_MODELS.find((def) => def.id === modelId);
}

export function buildPiModelsJson(
  provider: string,
  modelId: string,
  sessionId?: string
): string | undefined {
  if (provider !== OPENROUTER_PROVIDER) {
    return undefined;
  }
  const def = findPiRouterModelDef(provider, modelId);
  const config = {
    providers: {
      [OPENROUTER_PROVIDER]: {
        ...(sessionId !== undefined && {
          headers: { "x-session-id": sessionId },
        }),
        compat: {
          thinkingFormat: "openrouter",
          cacheControlFormat: "anthropic",
        },
        ...(def !== undefined && {
          models: [
            {
              id: def.id,
              name: def.name,
              reasoning: true,
              input: ["text"],
              cost: {
                input: def.costPerMillion.input,
                output: def.costPerMillion.output,
                cacheRead: def.costPerMillion.cacheRead,
                cacheWrite: def.costPerMillion.cacheWrite,
              },
              contextWindow: def.contextWindow,
              maxTokens: def.maxTokens,
              compat: {
                thinkingFormat: "openrouter",
                cacheControlFormat: "anthropic",
              },
            },
          ],
        }),
      },
    },
  };
  return JSON.stringify(config, null, 2);
}
