export function pyTypeName(value: unknown): string {
  if (value === null || value === undefined) {
    return "NoneType";
  }
  if (typeof value === "string") {
    return "str";
  }
  if (typeof value === "boolean") {
    return "bool";
  }
  if (typeof value === "number") {
    return Number.isInteger(value) ? "int" : "float";
  }
  if (Array.isArray(value)) {
    return "list";
  }
  return "dict";
}

export function pyRepr(value: unknown): string {
  if (value === null || value === undefined) {
    return "None";
  }
  if (typeof value === "string") {
    const escaped = value.replaceAll("\\", "\\\\");
    if (value.includes("'") && !value.includes('"')) {
      return `"${escaped}"`;
    }
    return `'${escaped.replaceAll("'", "\\'")}'`;
  }
  if (typeof value === "boolean") {
    return value ? "True" : "False";
  }
  if (typeof value === "number") {
    return String(value);
  }
  if (Array.isArray(value)) {
    return `[${value.map(pyRepr).join(", ")}]`;
  }
  return JSON.stringify(value);
}
