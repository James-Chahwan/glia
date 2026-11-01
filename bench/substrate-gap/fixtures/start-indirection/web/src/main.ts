import { createServer } from "http";

export function serve(port: number): void {
  createServer((_req, res) => res.end("ok")).listen(port);
}

serve(8080);
