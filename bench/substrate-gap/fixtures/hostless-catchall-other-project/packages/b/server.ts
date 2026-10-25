import express from 'express';
import path from 'path';

const app = express();

function serveIndex(_req: express.Request, res: express.Response) {
  res.sendFile(path.join(__dirname, 'index.html'));
}

app.get('/*', serveIndex);

app.listen(5173);
