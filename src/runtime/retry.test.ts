import { afterEach, describe, expect, it, spyOn } from "bun:test";

import {
  exit as exitEffect,
  fail,
  flatMap,
  retry,
  succeed,
  suspend,
} from "effect/Effect";

import { assertFailure } from "../../test/helpers/exit-asserts";
import { ModelError, SolverError } from "../harness/core";
import { runHarnessPromise } from "../internal/effect-logger";
import { getCurrentRetryAttempt } from "./response-cache";
import {
  rateLimitRetrySchedule,
  retrySalted,
  transientSolverRetrySchedule,
} from "./retry";

const schedule = rateLimitRetrySchedule({ maxRetries: 5, baseDelayMs: 0 });

const rateLimit = new ModelError({ status: 429, message: "429" });

const serverError = new ModelError({ status: 503, message: "503" });

const clientError = new ModelError({ status: 400, message: "400" });

const warnSpies: {
  mockRestore: () => void;
}[] = [];
afterEach(() => {
  for (const warn of warnSpies.splice(0)) {
    warn.mockRestore();
  }
});
describe("rateLimitRetrySchedule", () => {
  it("retries 429s until the effect succeeds", async () => {
    let attempts = 0;
    const flaky = suspend(() => {
      attempts++;
      return attempts < 3 ? fail(rateLimit) : succeed(attempts);
    });
    const result = await runHarnessPromise(flaky.pipe(retry(schedule)));
    expect(result).toBe(3);
  });
  it("logs one retry event for each retry and none for the terminal attempt", async () => {
    const warn = spyOn(console, "warn").mockImplementation(() => {});
    warnSpies.push(warn);
    let attempts = 0;
    const flaky = suspend(() => {
      attempts++;
      return attempts < 3 ? fail(rateLimit) : succeed(attempts);
    });
    await runHarnessPromise(flaky.pipe(retry(schedule)));
    expect(warn).toHaveBeenCalledTimes(2);
    expect(warn.mock.calls[0]?.[0]).toBe("Retrying after transient error");
    expect(warn.mock.calls[0]?.[1]).toMatchObject({
      attempt: 1,
      error_status: 429,
      error_tag: "ModelError",
    });
  });
  it("logs exactly maxRetries events before terminal exhaustion", async () => {
    const warn = spyOn(console, "warn").mockImplementation(() => {});
    warnSpies.push(warn);
    const program = suspend(() => fail(rateLimit)).pipe(
      retry(rateLimitRetrySchedule({ maxRetries: 2, baseDelayMs: 0 }))
    );
    const exit = await runHarnessPromise(program.pipe(exitEffect));
    assertFailure(exit);
    expect(warn).toHaveBeenCalledTimes(2);
  });
  it("caps retry error messages at 2000 characters", async () => {
    const warn = spyOn(console, "warn").mockImplementation(() => {});
    warnSpies.push(warn);
    const error = new ModelError({ status: 503, message: "x".repeat(2500) });
    const program = suspend(() => fail(error)).pipe(
      retry(rateLimitRetrySchedule({ maxRetries: 1, baseDelayMs: 0 }))
    );
    const exit = await runHarnessPromise(program.pipe(exitEffect));
    assertFailure(exit);
    const context = warn.mock.calls[0]?.[1];
    expect(context).toMatchObject({
      error_message: `${"x".repeat(1997)}...`,
    });
  });
  it("includes request identifiers in retry log fields", async () => {
    const warn = spyOn(console, "warn").mockImplementation(() => {});
    warnSpies.push(warn);
    const error = new ModelError({
      status: 503,
      message: "stream dropped",
      cfRay: "ray-123",
      xRequestId: "req-456",
      generationId: "gen-789",
    });
    const program = suspend(() => fail(error)).pipe(
      retry(rateLimitRetrySchedule({ maxRetries: 1, baseDelayMs: 0 }))
    );
    const exit = await runHarnessPromise(program.pipe(exitEffect));
    assertFailure(exit);
    expect(warn.mock.calls[0]?.[1]).toMatchObject({
      cf_ray: "ray-123",
      x_request_id: "req-456",
      generation_id: "gen-789",
    });
  });
  it("omits unavailable request identifiers from retry log fields", async () => {
    const warn = spyOn(console, "warn").mockImplementation(() => {});
    warnSpies.push(warn);
    const program = suspend(() =>
      fail(new ModelError({ status: 503, message: "stream dropped" }))
    ).pipe(retry(rateLimitRetrySchedule({ maxRetries: 1, baseDelayMs: 0 })));
    const exit = await runHarnessPromise(program.pipe(exitEffect));
    assertFailure(exit);
    expect(warn.mock.calls[0]?.[1]).not.toHaveProperty("cf_ray");
    expect(warn.mock.calls[0]?.[1]).not.toHaveProperty("x_request_id");
    expect(warn.mock.calls[0]?.[1]).not.toHaveProperty("generation_id");
  });
  it("retries transient 5xx errors", async () => {
    let attempts = 0;
    const flaky = suspend(() => {
      attempts++;
      return attempts < 3 ? fail(serverError) : succeed(attempts);
    });
    const result = await runHarnessPromise(flaky.pipe(retry(schedule)));
    expect(result).toBe(3);
  });
  it("does not retry non-retryable 4xx (other than 429)", async () => {
    let attempts = 0;
    const program = suspend(() => {
      attempts++;
      return fail(clientError);
    }).pipe(retry(schedule));
    const exit = await runHarnessPromise(program.pipe(exitEffect));
    assertFailure(exit);
    expect(attempts).toBe(1);
  });
  it("does not retry SolverError", async () => {
    let attempts = 0;
    const program = suspend(() => {
      attempts++;
      return fail(new SolverError({ message: "boom" }));
    }).pipe(retry(schedule));
    const exit = await runHarnessPromise(program.pipe(exitEffect));
    assertFailure(exit);
    expect(attempts).toBe(1);
  });
  it("gives up after maxRetries consecutive errors", async () => {
    let attempts = 0;
    const program = suspend(() => {
      attempts++;
      return fail(rateLimit);
    }).pipe(retry(rateLimitRetrySchedule({ maxRetries: 2, baseDelayMs: 0 })));
    const exit = await runHarnessPromise(program.pipe(exitEffect));
    assertFailure(exit);
    expect(attempts).toBe(3);
  });
});
describe("transientSolverRetrySchedule", () => {
  it("logs one retry event for each transient solver retry", async () => {
    const warn = spyOn(console, "warn").mockImplementation(() => {});
    warnSpies.push(warn);
    const solverError = new SolverError({ message: "sandbox unavailable" });
    let attempts = 0;
    const flaky = suspend(() => {
      attempts++;
      return attempts < 3 ? fail(solverError) : succeed(attempts);
    });
    await runHarnessPromise(
      flaky.pipe(
        retry(transientSolverRetrySchedule({ maxRetries: 3, baseDelayMs: 0 }))
      )
    );
    expect(warn).toHaveBeenCalledTimes(2);
    expect(warn.mock.calls[0]?.[1]).toMatchObject({
      attempt: 1,
      error_tag: "SolverError",
      error_message: "sandbox unavailable",
    });
  });
});
describe("retrySalted", () => {
  it("increments the retry-attempt salt on each retry and logs each retry", async () => {
    const warn = spyOn(console, "warn").mockImplementation(() => {});
    warnSpies.push(warn);
    const seen: (number | undefined)[] = [];
    let attempts = 0;
    const flaky = getCurrentRetryAttempt.pipe(
      flatMap((attempt) =>
        suspend(() => {
          seen.push(attempt);
          attempts++;
          return attempts < 3 ? fail(rateLimit) : succeed(attempts);
        })
      )
    );
    const result = await runHarnessPromise(
      retrySalted(
        flaky,
        rateLimitRetrySchedule({ maxRetries: 5, baseDelayMs: 0 })
      )
    );
    expect(result).toBe(3);
    expect(seen).toEqual([0, 1, 2]);
    expect(warn).toHaveBeenCalledTimes(2);
    expect(warn.mock.calls[0]?.[1]).toMatchObject({ attempt: 1 });
    expect(warn.mock.calls[1]?.[1]).toMatchObject({ attempt: 2 });
  });
  it("restarts the attempt salt at zero for each fresh call", async () => {
    const first = await runHarnessPromise(
      retrySalted(getCurrentRetryAttempt, rateLimitRetrySchedule({}))
    );
    const second = await runHarnessPromise(
      retrySalted(getCurrentRetryAttempt, rateLimitRetrySchedule({}))
    );
    expect(first).toBe(0);
    expect(second).toBe(0);
  });
});
