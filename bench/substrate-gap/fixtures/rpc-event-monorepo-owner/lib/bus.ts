// Root-level shared code (no nested manifest): unowned, so it pairs with every project.
import { EventEmitter } from "events";

export const bus = new EventEmitter();

export function registerAudit() {
  bus.on("orderPlaced", (order) => audit(order));
}

function audit(order: unknown) {
  console.log(order);
}
