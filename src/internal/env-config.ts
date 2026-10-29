import { ConfigProvider, fromEnv } from "effect/ConfigProvider";
import { getOrUndefined } from "effect/Context";
import type { Effect } from "effect/Effect";
import { provideService, withFiber } from "effect/Effect";

export function withLiveEnvConfig<A, E, R>(
  effect: Effect<A, E, R>
): Effect<A, E, R> {
  return withFiber((fiber) =>
    getOrUndefined(fiber.context, ConfigProvider) === undefined
      ? provideService(effect, ConfigProvider, fromEnv())
      : effect
  );
}
