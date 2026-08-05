export const unknownToString = (unk: unknown): string => {
  try {
    return JSON.stringify(unk) ?? String(unk);
  } catch {
    return `Not stringifiable: ${unk}`;
  }
};

export const unknownErrorToString = (rawError: unknown): string => {
  if (typeof rawError === "string") {
    return rawError;
  }
  if (rawError instanceof Error) {
    return rawError.message || rawError.name;
  }
  return unknownToString(rawError);
};
