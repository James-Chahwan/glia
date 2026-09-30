import { spawnSync } from "node:child_process";

export function deploy() {
  return spawnSync("shipit", ["deploy"], { stdio: "inherit" });
}
