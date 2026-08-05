import assert from "node:assert";

import type { Exit, Failure, Success } from "effect/Exit";
import { isFailure, isSuccess } from "effect/Exit";

export function assertSuccess<A, E>(
  exit: Exit<A, E>
): asserts exit is Success<A, E> {
  if (isSuccess(exit)) {
    return;
  }
  assert.fail("expected Exit to be Success");
}

export function assertFailure<A, E>(
  exit: Exit<A, E>
): asserts exit is Failure<A, E> {
  if (isFailure(exit)) {
    return;
  }
  assert.fail("expected Exit to be Failure");
}
