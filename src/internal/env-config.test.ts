import { afterEach, describe, expect, it } from "bun:test";

import { String as configString } from "effect/Config";
import { ConfigProvider, fromUnknown } from "effect/ConfigProvider";
import { provideService, runSync } from "effect/Effect";
import { getOrNull, isNone } from "effect/Option";

import { optionalEnv, withLiveEnvConfig } from "./env-config";

const ENV_NAME = "BENCH_HARNESS_ENV_CONFIG_TEST";

describe("optionalEnv", () => {
  afterEach(() => {
    delete process.env[ENV_NAME];
  });

  it("returns none when the variable is unset", () => {
    expect(isNone(runSync(optionalEnv(ENV_NAME)))).toBe(true);
  });

  it('keeps an empty string as Some("") like Effect 3 did', () => {
    process.env[ENV_NAME] = "";
    expect(getOrNull(runSync(optionalEnv(ENV_NAME)))).toBe("");
  });

  it("reads process.env live on every evaluation", () => {
    const read = optionalEnv(ENV_NAME);
    expect(isNone(runSync(read))).toBe(true);
    process.env[ENV_NAME] = "first";
    expect(getOrNull(runSync(read))).toBe("first");
    process.env[ENV_NAME] = "second";
    expect(getOrNull(runSync(read))).toBe("second");
  });
});

describe("withLiveEnvConfig", () => {
  it("keeps an explicitly provided ConfigProvider", () => {
    process.env[ENV_NAME] = "from-env";
    const explicit = fromUnknown({ [ENV_NAME]: "from-provider" });
    const value = runSync(
      provideService(
        withLiveEnvConfig(configString(ENV_NAME)),
        ConfigProvider,
        explicit
      )
    );
    expect(value).toBe("from-provider");
    delete process.env[ENV_NAME];
  });
});
