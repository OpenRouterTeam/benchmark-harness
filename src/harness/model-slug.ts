export function stripVariantSuffix(model: string): string {
  const idx = model.indexOf(":");
  return idx <= 0 ? model : model.slice(0, idx);
}
