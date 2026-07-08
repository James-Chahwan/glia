// Express server exposing the routes the client fetches.
import express from "express";

const app = express();

app.get("/users/:id", (req, res) => {
  res.json({ id: req.params.id });
});

app.post("/users", (req, res) => {
  res.status(201).json({ ok: true });
});

app.listen(8080);
