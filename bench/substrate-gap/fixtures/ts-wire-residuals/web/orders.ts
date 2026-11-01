import { Order } from './models';

export function loadOrder(id: number) {
  return fetch(`/orders/${id}`)
    .then((r) => r.json())
    .then((o: Order) => o);
}
