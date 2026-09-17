// Handles the orderPlaced event api/orders.ts emits.
import { bus } from "../api/orders";

export function registerHandlers() {
  bus.on("orderPlaced", (order) => {
    sendReceipt(order);
  });
}

function sendReceipt(order: { id: string }) {
  console.log(`receipt ${order.id}`);
}
