export async function listItems() {
  return fetch("/svc/items");
}

export async function deleteItem(id: number) {
  return fetch(`/svc/items/${id}`, { method: "DELETE" });
}

export async function recentOrders() {
  return fetch("/orders/recent");
}
