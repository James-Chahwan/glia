import { api } from "~/utils/api";

export function Users() {
  const { data } = api.user.list.useQuery();
  return (
    <ul>
      {data?.map((u) => (
        <li key={u.id}>{u.name}</li>
      ))}
    </ul>
  );
}
