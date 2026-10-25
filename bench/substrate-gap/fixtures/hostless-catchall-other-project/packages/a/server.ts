import express from 'express';

const app = express();

function h(_req: express.Request, res: express.Response) {
  res.json({ ok: true });
}

app.get('/x', h);

app.listen(3000);
