import { isRecord } from "../../internal/guards";
import { wLog } from "../../internal/log";
import type { Criterion } from "./schemas";

export function extractCriteria(answer: unknown): Criterion[] {
  if (!isRecord(answer)) {
    return [];
  }
  const criteria: Criterion[] = [];
  const sections = readArray(answer, "sections");
  for (const section of sections) {
    if (!isRecord(section)) {
      continue;
    }
    walk(
      section,
      String(section["id"] ?? "unknown"),
      String(section["title"] ?? "Unknown")
    );
  }
  return criteria;
  function walk(
    node: Record<string, unknown>,
    sectionId: string,
    sectionTitle: string
  ): void {
    for (const criterion of readArray(node, "criteria")) {
      if (!isRecord(criterion)) {
        continue;
      }
      const weight = criterion["weight"];
      const parsedWeight = typeof weight === "number" ? weight : Number(weight);
      if (!Number.isFinite(parsedWeight)) {
        wLog("draco criterion has non-numeric weight; defaulting to 0", {
          criterion_id: String(criterion["id"] ?? ""),
          weight,
        });
      }
      criteria.push({
        id: String(criterion["id"] ?? ""),
        section: sectionTitle,
        sectionId,
        weight: Number.isFinite(parsedWeight) ? parsedWeight : 0,
        requirement: String(criterion["requirement"] ?? ""),
      });
    }
    for (const subsection of readArray(node, "sections")) {
      if (!isRecord(subsection)) {
        continue;
      }
      walk(
        subsection,
        String(subsection["id"] ?? sectionId),
        String(subsection["title"] ?? sectionTitle)
      );
    }
  }
}

function readArray(
  node: Readonly<Record<string, unknown>>,
  key: string
): readonly unknown[] {
  const value = node[key];
  return Array.isArray(value) ? value : [];
}
