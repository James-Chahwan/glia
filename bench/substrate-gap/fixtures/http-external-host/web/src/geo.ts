import { environment } from './environments/environment';

export async function forwardGeocode(q: string) {
  return fetch(`https://nominatim.openstreetmap.org/search?q=${q}&format=json`);
}

export async function listProducts() {
  return fetch(`${environment.apiUrl}/products`);
}

export async function listOrders() {
  return fetch('https://api.shop.io/orders');
}
