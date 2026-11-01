import express from "express";

export function serve(): void {
  const app = express();
  app.listen(3000);
}

serve();
