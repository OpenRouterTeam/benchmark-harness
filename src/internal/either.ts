import type { Either as EitherType } from "./either-impl";
import { left, right } from "./either-impl";
export * as Either from "./either-impl";

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
