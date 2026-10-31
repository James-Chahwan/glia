import { createServer } from "http";

export function bootstrap(port: number): void {
  createServer((_req, res) => res.end("ok")).listen(port);
}

bootstrap(3000);
