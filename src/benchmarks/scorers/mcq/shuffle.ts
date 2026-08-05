function mulberry32(seed: number): () => number {
  let state = seed >>> 0;
  return () => {
    state = (state + 1831565813) | 0;
    let t = Math.imul(state ^ (state >>> 15), 1 | state);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

function mixSeed(seed: number): number {
  let x = (seed ^ 2654435769) >>> 0;
  x = Math.imul(x ^ (x >>> 16), 73244475) >>> 0;
  x = Math.imul(x ^ (x >>> 16), 73244475) >>> 0;
  return (x ^ (x >>> 16)) >>> 0;
}

export function seededPermutation(length: number, seed: number): number[] {
  const indices = Array.from({ length }, (_, i) => i);
  const random = mulberry32(mixSeed(seed));
  for (let i = length - 1; i > 0; i--) {
    const j = Math.floor(random() * (i + 1));
    const tmp = indices[i]!;
    indices[i] = indices[j]!;
    indices[j] = tmp;
  }
  return indices;
}
