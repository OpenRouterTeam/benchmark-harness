# Chess move ladder

A controlled multiple-choice benchmark for testing how a model's move-selection accuracy depends on the task-specific criteria supplied in its input. The same positions and candidate moves are rendered under a ladder of criteria-richness conditions so that "how much input engineering was done" is the manipulated variable and item-level pairing is preserved across conditions and models. It is the first battery of the System One (Jev) input-quality and constraint-solver study; ordinary chat models and typed System One models run on identical items.

## Source & license

- Dataset: <https://huggingface.co/datasets/Lichess/chess-puzzles> (`default` config, `train` split, ~6.1M rows), streamed through the Hugging Face rows endpoint.
- License: **CC0 1.0** (public domain), per the dataset card. The puzzles derive from Lichess games and the Lichess puzzle database, which Lichess publishes under CC0.
- Fields read: `PuzzleId`, `FEN`, `Moves`, `Rating`, `Themes`. Every field is validated at runtime and a malformed row throws a typed error.

## Item construction

`Moves` is a UCI sequence in which the first move is the opponent's setup move and the second is the first move of the puzzle solution. `puzzleRecordToItem` applies the setup move, treats the resulting position as the item, and treats the first solution move as the correct answer. Per the dataset card the puzzles come from Stockfish NNUE re-analysis (40 meganodes) of Lichess games, and the [lichess-puzzler](https://github.com/ornicar/lichess-puzzler) generator only emits positions where the solution move is the single move that keeps the winning evaluation, so the label is an engine-derived reference without a live engine call. Deeper continuation moves are not scored.

Candidates are the solution plus `candidateCount - 1` legal alternatives (default 4 total). Distractor selection is deterministic per `PuzzleId`: half of the distractors are the most "tempting" legal moves by a surface score (captured value, check, mate), the remainder are seeded-random legal moves. Letters are assigned after a seeded permutation so the target position is uniform.

Sample ids are `chess_move_ladder-<PuzzleId>`. Puzzle metadata (`Rating`, `Themes`) is retained in the sample for difficulty-stratified analysis.

## Criteria ladder (`criteriaLevel`)

| level | input contents |
| --- | --- |
| `board` | position and candidate list only; no statement of the goal |
| `instruction` | plus the goal: pick the objectively strongest move |
| `heuristics` | plus the six heuristics stated as rules (no computation done for the model) |
| `features` | plus, for every candidate, the value of each heuristic's feature precomputed with chess.js |
| `motif` | plus the Lichess tactical theme tags for the puzzle (a partial oracle signal) |

The distinction between `heuristics` and `features` is the computation ladder: the rule is identical, only whether its result is computed externally changes.

## Heuristics (`criteria.ts`)

`material`, `hanging`, `forcing`, `king_safety`, `threats`, `mobility`. Each has a `correct` and an `inverted` natural-language rule and a feature renderer. Feature values come from `computeCandidateFeatures` (material after the opponent's best immediate recapture, moved-piece safety, own/opponent hanging pieces, check and mate, king exposure, escape squares, highest attacked opponent piece, opponent mobility, center control). They are heuristic approximations of static exchange evaluation, not engine output.

- `heuristicsVariant: "inverted"` swaps every rule for its inversion (false-criterion injection) while leaving features untouched.
- `ablateCriterion` removes one criterion's rule and feature lines from `heuristics`/`features`/`motif` renderings.

## Model interfaces

- `chat` (default for ordinary models): a single user prompt ending in an `Answer: $LETTER` instruction, scored with the shared `extractMcqAnswer`.
- `systemone` (default for models matching `jev`): a typed System One request posted to `/systemone` by `makeSystemOneModelLayer`, with the board, goal, heuristics and motif in `state` and a single `choice` question whose criteria are the candidate letters (annotated with features at the `features`/`motif` levels). The returned probability vector and confidence are recorded in the score explanation.

`interface` overrides the default.

## Scoring

Exact match of the extracted letter against the solution letter. The score explanation is JSON with `target`, `extracted`, an `error` when extraction failed (distinguishing it from a wrong answer), and `pCorrect`, `probabilities` and `confidence` when the provider exposes them.

## Config options

| option | type | default |
| --- | --- | --- |
| `criteriaLevel` | `board` \| `instruction` \| `heuristics` \| `features` \| `motif` | `instruction` |
| `heuristicsVariant` | `correct` \| `inverted` | `correct` |
| `ablateCriterion` | one of the six criterion ids | unset |
| `boardFormat` | `fen` \| `ascii` \| `both` | `both` |
| `candidateCount` | integer 2–8 | `4` |
| `interface` | `chat` \| `systemone` | derived from the model id |

Temperature is fixed at 0.

## Validity notes

- Chance accuracy is `1 / candidateCount`.
- The tempting-distractor rule makes the `forcing` and `material` surface cues frequently point at a wrong candidate, which is intended for cue-conflict analysis but means accuracy is not comparable to raw puzzle solve rates.
- Feature values are heuristic and can disagree with an engine on tactically deep positions; they are inputs to the model, not the scoring reference.
- No published target score exists for this construction. Calibrate against the ceiling condition (`motif`) and against chance.

## Smoke run

First 20 puzzles, `candidateCount=4` (chance 0.25), `boardFormat=both`, temperature 0, `--reasoning-effort low`. Point estimates only; n=20 is far below the item count needed for the interaction test.

| model                           | `board` | `heuristics` | `features` |
| ------------------------------- | ------- | ------------ | ---------- |
| `typesafe/jev-1.13` (systemone) | 0.40    | 0.35         | 0.70       |
| `openai/gpt-4o-mini` (chat)     | 0.50    | -            | 0.55       |

Jev answered every item with a parseable choice distribution; the chat model produced no extraction failures.
