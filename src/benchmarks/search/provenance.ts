import { createHash } from "node:crypto";

import {
  BROWSECOMP_ROWS,
  BROWSECOMP_SHA256,
  BROWSECOMP_URL,
} from "./browsecomp/dataset";
import {
  DEEP_RESEARCH_INSTRUCTIONS,
  WIDESEARCH_INSTRUCTIONS,
} from "./core/prompts";
import {
  DSQA_DATASET_REVISION,
  DSQA_DATASET_ROWS,
  DSQA_DATASET_SHA256,
  DSQA_DATASET_URL,
} from "./dsqa/dataset";
import {
  DSQA_GRADER_PROMPT,
  DSQA_JUDGE_CONFIG,
  DSQA_JUDGE_PROMPT_ID,
} from "./dsqa/grader";
import {
  ANSWER_EQUIVALENCE_GRADER_PROMPT,
  ANSWER_EQUIVALENCE_JUDGE_CONFIG,
  ANSWER_EQUIVALENCE_JUDGE_PROMPT_ID,
} from "./grading/answer-equivalence";
import {
  WIDESEARCH_DATASET_SHA256,
  WIDESEARCH_DATASET_URL,
  WIDESEARCH_REVISION,
  WIDESEARCH_ROWS,
} from "./widesearch/dataset";
import { WIDESEARCH_GRADING_REFERENCE_DATE } from "./widesearch/grading";
import {
  ALIGNMENT_PROMPT,
  CELL_JUDGE_PROMPT,
  WIDESEARCH_ALIGNMENT_PROMPT_ID,
  WIDESEARCH_CELL_JUDGE_PROMPT_ID,
  WIDESEARCH_JUDGE_CONFIG,
} from "./widesearch/judges";

export type SearchBenchmarkProvenanceId =
  | "search_browsecomp"
  | "search_dsqa"
  | "search_widesearch";

export interface SearchBenchmarkProvenance {
  readonly benchmarkId: SearchBenchmarkProvenanceId;
  readonly dataset: {
    readonly source: string;
    readonly revision: string | null;
    readonly sha256: string;
    readonly rowCount: number;
  };
  readonly generationPrompt: {
    readonly sha256: string;
  };
  readonly judges: readonly {
    readonly model: string;
    readonly prompt: {
      readonly id: string;
      readonly sha256: string;
    };
  }[];
  readonly gradingReferenceDate: string | null;
}

const SEARCH_BENCHMARK_PROVENANCE = {
  search_browsecomp: {
    benchmarkId: "search_browsecomp",
    dataset: {
      source: BROWSECOMP_URL,
      revision: null,
      sha256: BROWSECOMP_SHA256,
      rowCount: BROWSECOMP_ROWS,
    },
    generationPrompt: { sha256: sha256(DEEP_RESEARCH_INSTRUCTIONS) },
    judges: [
      {
        model: ANSWER_EQUIVALENCE_JUDGE_CONFIG.judgeModel,
        prompt: {
          id: ANSWER_EQUIVALENCE_JUDGE_PROMPT_ID,
          sha256: sha256(ANSWER_EQUIVALENCE_GRADER_PROMPT),
        },
      },
    ],
    gradingReferenceDate: null,
  },
  search_dsqa: {
    benchmarkId: "search_dsqa",
    dataset: {
      source: DSQA_DATASET_URL,
      revision: DSQA_DATASET_REVISION,
      sha256: DSQA_DATASET_SHA256,
      rowCount: DSQA_DATASET_ROWS,
    },
    generationPrompt: { sha256: sha256(DEEP_RESEARCH_INSTRUCTIONS) },
    judges: [
      {
        model: DSQA_JUDGE_CONFIG.judgeModel,
        prompt: {
          id: DSQA_JUDGE_PROMPT_ID,
          sha256: sha256(DSQA_GRADER_PROMPT),
        },
      },
    ],
    gradingReferenceDate: null,
  },
  search_widesearch: {
    benchmarkId: "search_widesearch",
    dataset: {
      source: WIDESEARCH_DATASET_URL,
      revision: WIDESEARCH_REVISION,
      sha256: WIDESEARCH_DATASET_SHA256,
      rowCount: WIDESEARCH_ROWS,
    },
    generationPrompt: { sha256: sha256(WIDESEARCH_INSTRUCTIONS) },
    judges: [
      {
        model: WIDESEARCH_JUDGE_CONFIG.judgeModel,
        prompt: {
          id: WIDESEARCH_ALIGNMENT_PROMPT_ID,
          sha256: sha256(ALIGNMENT_PROMPT),
        },
      },
      {
        model: WIDESEARCH_JUDGE_CONFIG.judgeModel,
        prompt: {
          id: WIDESEARCH_CELL_JUDGE_PROMPT_ID,
          sha256: sha256(CELL_JUDGE_PROMPT),
        },
      },
    ],
    gradingReferenceDate: WIDESEARCH_GRADING_REFERENCE_DATE,
  },
} as const satisfies Readonly<
  Record<SearchBenchmarkProvenanceId, SearchBenchmarkProvenance>
>;

export function getSearchBenchmarkProvenance(
  id: string
): SearchBenchmarkProvenance | undefined {
  return isSearchBenchmarkProvenanceId(id)
    ? SEARCH_BENCHMARK_PROVENANCE[id]
    : undefined;
}

export function searchBenchmarkProvenanceIds(): readonly SearchBenchmarkProvenanceId[] {
  return Object.keys(
    SEARCH_BENCHMARK_PROVENANCE
  ) as SearchBenchmarkProvenanceId[];
}

function isSearchBenchmarkProvenanceId(
  id: string
): id is SearchBenchmarkProvenanceId {
  return Object.hasOwn(SEARCH_BENCHMARK_PROVENANCE, id);
}

function sha256(value: string): string {
  return createHash("sha256").update(value, "utf8").digest("hex");
}
