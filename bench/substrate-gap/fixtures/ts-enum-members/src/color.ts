export enum Color {
  Red = 'red',
  Green = 'green',
  Blue = 'blue',
}

export const enum Dir {
  Up,
  Down,
}

enum Local {
  A = 1,
  B,
}

export function warm(c: Color): boolean {
  return c === Color.Red;
}

export function firstLocal(): Local {
  return Local.A;
}
