import type { Effect } from "effect/Effect";
import { fail } from "effect/Effect";
import type { Layer } from "effect/Layer";
import { unwrap } from "effect/Layer";

export function layerFail<E, ROut = never>(error: E): Layer<ROut, E> {
  const failing: Effect<Layer<ROut, E>, E> = fail(error);
  return unwrap(failing);
}
