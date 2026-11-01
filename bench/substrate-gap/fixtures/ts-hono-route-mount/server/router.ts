import { Hono } from 'hono';
import { cors } from 'hono/cors';

import { auth } from './api/auth';
import { filesRoute } from './api/files/files';

const app = new Hono().basePath(process.env.BASE_PATH || '/');

app.use('/svc/*', cors());

// Auth server.
app.route('/svc/auth', auth);

// Files route.
app.route('/svc/files', filesRoute);

app.get('/healthz', (c) => c.text('ok'));

export default app;
