import { formatCents } from "@shop/money";
import { clamp } from "utils";

export function refund(c) {
  return formatCents(clamp(c, 0, 100000));
}
