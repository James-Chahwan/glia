// IMPORTS probe (TypeScript): main.ts imports ./util (relative). KNOWN STUBBED.
import { greet } from "./util";

export function run(): void {
  console.log(greet("world"));
}
