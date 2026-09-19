import { api } from "./api";

export function Users() {
  const users = api.user.list.useQuery();
  return users;
}
