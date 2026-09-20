export const PUZZLE_RECORD = {
  PuzzleId: "00008",
  GameId: "787zsVup/black#48",
  FEN: "r6k/pp2r2p/4Rp1Q/3p4/8/1N1P2R1/PqP2bPP/7K b - - 0 24",
  Moves: "f2g3 e6e7 b2b1 b3c1 b1c1 h6c1",
  Rating: 1939,
  RatingDeviation: 77,
  Popularity: 95,
  NbPlays: 10_000,
  Themes: ["crushing", "hangingPiece", "long", "middlegame"],
  OpeningTags: null,
} as const;

export const TWO_MATES_RECORD = {
  PuzzleId: "twomates",
  FEN: "7k/1R6/6K1/8/8/8/8/3Q4 b - - 0 1",
  Moves: "h8g8 d1d8",
  Rating: 500,
  Themes: ["mate", "mateIn1"],
} as const;

export const MATE_IN_ONE_RECORD = {
  PuzzleId: "mate1",
  FEN: "r1bqkb1r/pppp1ppp/2n2n2/4p2Q/2B1P3/8/PPPP1PPP/RNB1K1NR b KQkq - 0 4",
  Moves: "a7a6 h5f7",
  Rating: 600,
  Themes: ["mate", "mateIn1", "oneMove"],
} as const;
