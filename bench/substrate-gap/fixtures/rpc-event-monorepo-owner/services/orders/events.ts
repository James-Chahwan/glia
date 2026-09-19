// The orders service's own in-process bus.
import { EventEmitter } from "events";

const bus = new EventEmitter();

export function placeOrder(id: string) {
  bus.emit("orderPlaced", { id });
}

export function registerOrderHandlers() {
  bus.on("orderPlaced", (order) => ship(order));
}

function ship(order: unknown) {
  console.log(order);
}
