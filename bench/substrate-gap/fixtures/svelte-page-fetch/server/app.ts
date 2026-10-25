import express from "express";

const app = express();

app.get("/api/albums", (_req, res) => {
  res.json([]);
});

app.get("/api/users", (_req, res) => {
  res.json([]);
});

app.listen(3000);
