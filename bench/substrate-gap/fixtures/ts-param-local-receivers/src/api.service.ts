export class ApiService {
  fetchUser(id: number): string {
    return `u${id}`;
  }

  purge(): void {}
}
