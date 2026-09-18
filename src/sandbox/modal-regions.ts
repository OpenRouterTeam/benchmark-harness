import { stripRoutingPrefix } from "../harness/model-slug";

export const MODAL_US_REGIONS = ["us"] as const;

export interface ModalRegionPin {
  readonly modelPrefix: string;
  readonly regions: readonly string[];
}

export const MODAL_REGION_PINS: readonly ModalRegionPin[] = [
  { modelPrefix: "sakana/fugu-", regions: MODAL_US_REGIONS },
];

export function resolveModalRegions(
  model: string,
  configured: readonly string[] | undefined,
  pins: readonly ModalRegionPin[] = MODAL_REGION_PINS
): readonly string[] | undefined {
  if (configured !== undefined) {
    return configured;
  }
  const slug = stripRoutingPrefix(model);
  return pins.find((pin) => slug.startsWith(pin.modelPrefix))?.regions;
}
