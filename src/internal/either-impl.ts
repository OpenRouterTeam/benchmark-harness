class LeftImpl<L> {
  declare readonly _tag: "Left";
  readonly left: L;

  constructor(left: L) {
    this.left = left;
  }
}

class RightImpl<R> {
  declare readonly _tag: "Right";
  readonly right: R;

  constructor(right: R) {
    this.right = right;
  }
}

Object.defineProperty(LeftImpl.prototype, "_tag", { value: "Left" });
Object.defineProperty(RightImpl.prototype, "_tag", { value: "Right" });

export type Left<L> = LeftImpl<L>;

export type Right<R> = RightImpl<R>;

export type Either<R, L = never> = Left<L> | Right<R>;

export function left<L>(value: L): Either<never, L> {
  return new LeftImpl(value);
}

export function right<R>(value: R): Either<R, never> {
  return new RightImpl(value);
}

export function isLeft<R, L>(self: Either<R, L>): self is Left<L> {
  return self._tag === "Left";
}

export function isRight<R, L>(self: Either<R, L>): self is Right<R> {
  return self._tag === "Right";
}

function tryEither<R, L>(options: {
  readonly try: () => R;
  readonly catch: (error: unknown) => L;
}): Either<R, L>;
function tryEither<R>(evaluate: () => R): Either<R, unknown>;
function tryEither<R, L>(
  input:
    | (() => R)
    | { readonly try: () => R; readonly catch: (error: unknown) => L }
): Either<R, L | unknown> {
  if (typeof input === "function") {
    try {
      return right(input());
    } catch (error) {
      return left(error);
    }
  }
  try {
    return right(input.try());
  } catch (error) {
    return left(input.catch(error));
  }
}

export { tryEither as try };

export function getOrThrow<R, L>(self: Either<R, L>): R {
  if (isRight(self)) {
    return self.right;
  }
  throw new Error("getOrThrow called on a Left");
}
