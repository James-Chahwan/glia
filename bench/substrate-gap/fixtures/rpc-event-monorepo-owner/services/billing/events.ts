// The billing service's own in-process bus: same event name, another process.
import { EventEmitter } from "events";

const bus = new EventEmitter();

export function chargeOrder(id: string) {
  bus.emit("orderPlaced", { id });
}

export function registerBillingHandlers() {
  bus.on("orderPlaced", (order) => invoice(order));
}

function invoice(order: unknown) {
  console.log(order);
}
