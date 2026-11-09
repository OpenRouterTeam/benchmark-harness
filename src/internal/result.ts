import type { Result } from "effect/Result";
import { fail, succeed } from "effect/Result";

export type AsyncResult<A, E = unknown> = Promise<Result<A, E>>;

export async function tryPromiseResult<A>(
  thunk: () => Promise<A>
): AsyncResult<A> {
  try {
    return succeed(await thunk());
  } catch (error) {
    return fail(error);
  }
}
