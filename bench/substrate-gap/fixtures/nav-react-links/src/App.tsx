import { createBrowserRouter, Link, NavLink, useNavigate } from 'react-router-dom';
import { Dashboard, Settings, User, NotFound } from './pages';

export const router = createBrowserRouter([
  { path: '/dashboard', element: <Dashboard /> },
  { path: '/settings', element: <Settings /> },
  { path: '/users/:id', element: <User /> },
  { path: '*', element: <NotFound /> },
]);

export function Nav({ id }: { id: number }) {
  const navigate = useNavigate();
  return (
    <div>
      <Link to="/dashboard">D</Link>
      <NavLink to={`/users/${id}`}>U</NavLink>
      <button onClick={() => navigate("/settings")}>S</button>
      <Link to="/billing">B</Link>
      <a href="/auth/google">G</a>
    </div>
  );
}
