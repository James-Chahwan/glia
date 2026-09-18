// In-process bus (Node EventEmitter held in a bus-shaped name): the control.
import { EventEmitter } from "events";

export const bus = new EventEmitter();

export function placeOrder(id: string) {
  bus.emit("orderPlaced", { id });
}
