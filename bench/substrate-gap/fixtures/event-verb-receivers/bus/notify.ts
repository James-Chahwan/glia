import { bus } from "./orders";

export function registerHandlers() {
  bus.on("orderPlaced", (order) => {
    console.log(order);
  });
}
