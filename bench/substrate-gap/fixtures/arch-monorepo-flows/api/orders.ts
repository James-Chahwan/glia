// In-process bus: api emits orderPlaced, worker handles it (EVENT_FLOWS).
import { EventEmitter } from "events";

export const bus = new EventEmitter();

export function placeOrder(id: string) {
  bus.emit("orderPlaced", { id });
}
