import { useEffect, useState } from "react";

// React component issuing a fetch() to a backend route (server dir).
// The enclosing function CALLS the ENDPOINT node; HttpStackResolver should
// then pair that ENDPOINT with the Go ROUTE across the client/server boundary.
export function UserList() {
  const [users, setUsers] = useState<unknown[]>([]);

  useEffect(() => {
    fetch("/users")
      .then((r) => r.json())
      .then(setUsers);
  }, []);

  const addUser = async (body: unknown) => {
    await fetch("/users", { method: "POST", body: JSON.stringify(body) });
  };

  return null;
}
