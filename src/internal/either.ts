import type { Either as EitherType } from "effect/Either";
import { left, right } from "effect/Either";
export * as Either from "effect/Either";

export type AsyncEither<A, E = unknown> = Promise<EitherType<A, E>>;

export async function tryPromiseEither<A>(
  thunk: () => Promise<A>
): AsyncEither<A> {
  try {
    return right(await thunk());
  } catch (error) {
    return left(error);
  }
}
