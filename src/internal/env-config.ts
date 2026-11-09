import type { ConfigError } from "effect/Config";
import { option, String as configString } from "effect/Config";
import { ConfigProvider, fromEnv } from "effect/ConfigProvider";
import { getOrUndefined } from "effect/Context";
import type { Effect } from "effect/Effect";
import { provideService, withFiber } from "effect/Effect";
import type { Option } from "effect/Option";

export function withLiveEnvConfig<A, E, R>(
  effect: Effect<A, E, R>
): Effect<A, E, R> {
  return withFiber((fiber) =>
    getOrUndefined(fiber.context, ConfigProvider) === undefined
      ? provideService(
          effect,
          ConfigProvider,
          fromEnv({ preserveEmptyStrings: true })
        )
      : effect
  );
}

export function optionalEnv(name: string): Effect<Option<string>, ConfigError> {
  return withLiveEnvConfig(configString(name).pipe(option));
}
