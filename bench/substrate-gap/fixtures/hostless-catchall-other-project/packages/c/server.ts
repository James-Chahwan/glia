import express from 'express';

const app = express();

function serveShell(_req: express.Request, res: express.Response) {
  res.send('<html></html>');
}

app.get('/*', serveShell);

app.listen(4000);
