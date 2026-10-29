import * as Result from "effect/Result";

export function assertSuccess<A, E>(
  r: Result.Result<A, E>
): asserts r is Result.Success<A, E> {
  if (Result.isFailure(r)) {
    throw new Error(
      `Expected Success, got Failure: ${JSON.stringify(r.failure)}`
    );
  }
}

export function assertFailure<A, E>(
  r: Result.Result<A, E>
): asserts r is Result.Failure<A, E> {
  if (Result.isSuccess(r)) {
    throw new Error(
      `Expected Failure, got Success: ${JSON.stringify(r.success)}`
    );
  }
}
