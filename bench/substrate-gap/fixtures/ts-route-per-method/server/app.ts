import express from "express";

const app = express();

export function listUsers(req: any, res: any) { res.json([]); }
export function createUser(req: any, res: any) { res.status(201).json({}); }
export function getUser(req: any, res: any) { res.json({}); }
export function deleteUser(req: any, res: any) { res.status(204).end(); }

app.get("/users", listUsers);
app.post("/users", createUser);
app.get("/users/:id", getUser);
app.delete("/users/:id", deleteUser);
app.all("/health", (req: any, res: any) => res.send("ok"));

app.listen(8080);
