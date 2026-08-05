export interface ModelErrorIdentifiers {
  readonly cfRay?: string;
  readonly xRequestId?: string;
  readonly generationId?: string;
}

export function pickModelErrorIdentifiers(
  source: ModelErrorIdentifiers
): ModelErrorIdentifiers {
  return {
    ...(source.cfRay !== undefined && { cfRay: source.cfRay }),
    ...(source.xRequestId !== undefined && { xRequestId: source.xRequestId }),
    ...(source.generationId !== undefined && {
      generationId: source.generationId,
    }),
  };
}

export function appendModelErrorIdentifiers(
  message: string,
  identifiers: ModelErrorIdentifiers
): string {
  const fields = [
    identifiers.cfRay !== undefined ? `cf_ray=${identifiers.cfRay}` : undefined,
    identifiers.xRequestId !== undefined
      ? `x_request_id=${identifiers.xRequestId}`
      : undefined,
    identifiers.generationId !== undefined
      ? `generation_id=${identifiers.generationId}`
      : undefined,
  ].filter((field): field is string => field !== undefined);
  return fields.length > 0 ? `${message} (${fields.join(", ")})` : message;
}

export function modelErrorIdentifiersFromHeaders(
  headers: Readonly<Record<string, string>>
): Pick<ModelErrorIdentifiers, "cfRay" | "xRequestId"> {
  const valueFor = (name: string): string | undefined => {
    const entry = Object.entries(headers).find(
      ([key]) => key.toLowerCase() === name
    );
    return entry?.[1] || undefined;
  };
  return {
    ...(valueFor("cf-ray") !== undefined && { cfRay: valueFor("cf-ray") }),
    ...(valueFor("x-request-id") !== undefined && {
      xRequestId: valueFor("x-request-id"),
    }),
  };
}

export function modelErrorIdentifiersFromFetchHeaders(
  headers: Pick<Headers, "get">
): Pick<ModelErrorIdentifiers, "cfRay" | "xRequestId"> {
  const cfRay = headers.get("cf-ray") || undefined;
  const xRequestId = headers.get("x-request-id") || undefined;
  return {
    ...(cfRay !== undefined && { cfRay }),
    ...(xRequestId !== undefined && { xRequestId }),
  };
}
