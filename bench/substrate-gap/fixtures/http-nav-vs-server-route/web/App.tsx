import axios from "axios";
import { Routes, Route } from "react-router-dom";

// The SPA renders a /users page (browser navigation) ...
function Users() {
  return <div>users</div>;
}

export default function App() {
  return (
    <Routes>
      <Route path="/users" element={<Users />} />
    </Routes>
  );
}

// ... and fetches /users from the backend in api/app.py.
export async function load() {
  return axios.get('/users');
}
