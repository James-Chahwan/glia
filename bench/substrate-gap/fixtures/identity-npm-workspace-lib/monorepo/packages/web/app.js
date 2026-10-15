import { formatCents } from "@shop/money";

export function price(c) {
  return "$" + formatCents(c);
}
