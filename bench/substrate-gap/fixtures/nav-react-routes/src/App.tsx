import { Routes, Route, Navigate } from 'react-router-dom';
import { Settings, ProfilePage, Home } from './pages';

export function App() {
  return (
    <Routes>
      <Route path="/" element={<Home />} />
      <Route path="/settings" element={<Settings />}>
        <Route path="profile" element={<ProfilePage />} />
        <Route index element={<Settings />} />
      </Route>
      <Route path="/old" element={<Navigate to="/reports" />} />
    </Routes>
  );
}
