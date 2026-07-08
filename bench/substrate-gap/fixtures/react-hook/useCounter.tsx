import { useState, useCallback } from "react";

// Custom hook DEFINED here.
export function useCounter(initial: number) {
  const [count, setCount] = useState(initial);
  const increment = useCallback(() => setCount((c) => c + 1), []);
  return { count, increment };
}

// Component that USES the custom hook (returns JSX — a real component).
export function Counter() {
  const { count, increment } = useCounter(0);
  return <button onClick={increment}>{count}</button>;
}
