export type ValueOf<T> = T extends readonly (infer U)[] ? U : T[keyof T];

type OmitValue<T, TV> = {
  [K in keyof T as T[K] extends TV ? never : K]: Exclude<T[K], TV>;
};

export function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

export function isMember<T extends string | number>(
  x: string | number | null | undefined,
  obj: Record<string, T> | readonly T[]
): x is T {
  return (Array.isArray(obj) ? obj : Object.values(obj)).includes(x);
}

export function isDefinedAndNotNull<T>(
  value: T | null | undefined
): value is T {
  return value !== undefined && value !== null;
}

export function definedValues<TK extends string, T extends Record<TK, unknown>>(
  obj: T
): OmitValue<T, undefined> {
  return Object.fromEntries(
    Object.entries(obj).filter(([, v]) => v !== undefined)
  ) as OmitValue<T, undefined>;
}
