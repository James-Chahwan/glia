// CALLS probe (TypeScript): compute() calls helper().
function helper(x: number): number {
  return x * 2;
}

export function compute(n: number): number {
  return helper(n) + 1;
}
