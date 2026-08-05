import { Either } from "./either";

export function assertRight<A, E>(
  e: Either.Either<A, E>
): asserts e is Either.Right<E, A> {
  if (Either.isLeft(e)) {
    throw new Error(`Expected Right, got Left: ${JSON.stringify(e.left)}`);
  }
}

export function assertLeft<A, E>(
  e: Either.Either<A, E>
): asserts e is Either.Left<E, A> {
  if (Either.isRight(e)) {
    throw new Error(`Expected Left, got Right: ${JSON.stringify(e.right)}`);
  }
}
