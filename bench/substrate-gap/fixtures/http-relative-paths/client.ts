export async function loadWidgets() {
  return fetch('widgets');
}

export async function loadBolts() {
  return fetch('/bolts');
}

export async function loadItems() {
  return fetch('items');
}
