import Link from 'next/link';
import { useRouter } from 'next/router';

export default function Home() {
  const router = useRouter();
  return (
    <main>
      <Link href="/users/7">U</Link>
      <button onClick={() => router.push("/orders/9")}>O</button>
      <Link href="/docs/intro">Docs</Link>
    </main>
  );
}
