import express, { Request, Response } from "express";

const app = express();

export function getUser(req: Request, res: Response): void {
  res.json({ id: req.params.id });
}

export function createUser(req: Request, res: Response): void {
  res.status(201).json({ ok: true });
}

app.get("/users/:id", getUser);
app.post("/users", createUser);

app.listen(8080);
