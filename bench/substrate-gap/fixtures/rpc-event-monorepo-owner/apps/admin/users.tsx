import { api } from "./api";

export function AdminUsers() {
  const users = api.user.list.useQuery();
  return users;
}
