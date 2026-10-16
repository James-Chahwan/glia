export class User {
  constructor(public id: string, public email: string) {}
}
export function displayName(u: User): string { return u.email; }
