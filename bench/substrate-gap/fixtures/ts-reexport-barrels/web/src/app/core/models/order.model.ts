import { User } from './user.model';
export class Order {
  constructor(public id: string, public cents: number) {}
}
export function orderTotal(orders: Order[]): number { return orders.reduce((s, o) => s + o.cents, 0); }
export function displayName(o: Order): string { return o.id; }
export { User as Customer };
