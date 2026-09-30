export const PROBABLY_PROGRAM_IDS = ["sentinel_case_v1"] as const;

export type ProbablyProgramId = (typeof PROBABLY_PROGRAM_IDS)[number];

export const DEFAULT_PROBABLY_PROGRAM: ProbablyProgramId = "sentinel_case_v1";

export const DECISION_ACTIONS = [
  "hold",
  "frontier_block",
  "inference_block",
  "account_ban",
  "key_revocation",
  "throttle",
] as const;

export type DecisionAction = (typeof DECISION_ACTIONS)[number];

export const LEAKED_KEY_LABEL =
  "the traffic is driven by a leaked, harvested or replayed API key draining a legitimate account holder, rather than by the operator of the account";

export const CORROBORATED_LABEL =
  "the coordination is corroborated by a second account-owned signal independent of relay or client uniformity, such as the accounts' own funding events, spend, declines, overdraft or a shared card fingerprint";

export const COHERENT_LABEL =
  "the account's identity is coherent and its charges are clearing, so this reads as a review request or false positive rather than a fraud finding";

export const COORDINATED_LABEL =
  "several accounts acting together in a shared behavioral pattern, such as identical request shapes, synchronized bursts, shared funding or one operator behind many logins";

export const STATIC_ONLY_LABEL =
  "accounts that merely share a static attribute such as an email domain, card BIN, IP hash, ASN or TLS fingerprint, with no shared behavior demonstrated";

export const LONE_LABEL =
  "a single account acting alone, with no other actors implicated";

export const LOAD_LABEL =
  "an identifiable customer whose traffic is legitimate but too heavy or too fast, where a rate limit or spend cap is the proportionate response";

export const FRONTIER_LABEL =
  "abuse that a block on frontier US models would stop, because the harm is concentrated in spend on frontier models";

export const ACCOUNT_WIDE_LABEL =
  "free-tier farming, mass registration, credit stockpiling, overdraft or non-frontier abuse that a frontier block would not touch, warranting an account-wide inference block";

export const BAN_LABEL =
  "conduct that requires a full account ban, such as prohibited content, chargeback fraud, provider abuse reports or repeat offense after a prior restriction";

const REMEDY_MATCH = `
      match dossier {
        ${JSON.stringify(FRONTIER_LABEL)} => {
          action = "frontier_block"
        }
        ${JSON.stringify(ACCOUNT_WIDE_LABEL)} => {
          action = "inference_block"
        }
        ${JSON.stringify(BAN_LABEL)} => {
          action = "account_ban"
        }
      }`;

const SENTINEL_CASE_V1 = `
let dossier = input()
let action = "hold"

if dossier feels ${JSON.stringify(LEAKED_KEY_LABEL)} with confidence 70% {
  action = "key_revocation"
} otherwise maybe {
  action = "hold"
} else {
  match dossier {
    ${JSON.stringify(LOAD_LABEL)} => {
      action = "throttle"
    }
    ${JSON.stringify(COORDINATED_LABEL)} => {
      if dossier feels ${JSON.stringify(CORROBORATED_LABEL)} with confidence 70% {${REMEDY_MATCH}
      } otherwise maybe {
        action = "hold"
      } else {
        action = "hold"
      }
    }
    ${JSON.stringify(STATIC_ONLY_LABEL)} => {
      action = "hold"
    }
    ${JSON.stringify(LONE_LABEL)} => {
      if dossier feels ${JSON.stringify(COHERENT_LABEL)} with confidence 70% {
        action = "hold"
      } otherwise maybe {
        action = "hold"
      } else {${REMEDY_MATCH}
      }
    }
  }
}

print(action)
`;

const PROGRAMS: Readonly<Record<ProbablyProgramId, string>> = {
  sentinel_case_v1: SENTINEL_CASE_V1,
};

export function probablyProgramSource(id: ProbablyProgramId): string {
  return PROGRAMS[id];
}

export interface BranchExpectation {
  readonly label: string;
  readonly expect: "chosen" | "rejected";
}

export const REMEDY_LABELS: Readonly<
  Record<
    Exclude<DecisionAction, "hold" | "key_revocation" | "throttle">,
    string
  >
> = {
  frontier_block: FRONTIER_LABEL,
  inference_block: ACCOUNT_WIDE_LABEL,
  account_ban: BAN_LABEL,
};

export function branchExpectations(
  action: DecisionAction
): readonly BranchExpectation[] {
  switch (action) {
    case "key_revocation": {
      return [{ label: LEAKED_KEY_LABEL, expect: "chosen" }];
    }
    case "hold": {
      return [];
    }
    case "throttle": {
      return [
        { label: LEAKED_KEY_LABEL, expect: "rejected" },
        { label: LOAD_LABEL, expect: "chosen" },
      ];
    }
    case "frontier_block":
    case "inference_block":
    case "account_ban": {
      return [
        { label: LEAKED_KEY_LABEL, expect: "rejected" },
        { label: STATIC_ONLY_LABEL, expect: "rejected" },
        { label: LOAD_LABEL, expect: "rejected" },
        { label: COHERENT_LABEL, expect: "rejected" },
        { label: REMEDY_LABELS[action], expect: "chosen" },
      ];
    }
    default: {
      return action satisfies never;
    }
  }
}

export function isDecisionAction(value: string): value is DecisionAction {
  return (DECISION_ACTIONS as readonly string[]).includes(value);
}
