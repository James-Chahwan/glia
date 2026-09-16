import { Routes, Route } from "react-router-dom";
import { loadDashboard } from "./api";

function Dashboard() {
  return <div>dashboard</div>;
}

export default function App() {
  return (
    <Routes>
      <Route path="/dashboard" element={<Dashboard />} />
    </Routes>
  );
}
