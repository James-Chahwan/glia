// A Node EventEmitter declared in this file under a non-bus name.
import { EventEmitter } from "node:events";

const jobs = new EventEmitter();

export function watchJobs(report: () => void) {
  jobs.on("drained", report);
}

export function drain() {
  jobs.emit("drained");
}
