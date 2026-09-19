export function fmt(x: number): string {
  return pad(x);
}

function pad(x: number): string {
  return String(x).padStart(4, "0");
}
