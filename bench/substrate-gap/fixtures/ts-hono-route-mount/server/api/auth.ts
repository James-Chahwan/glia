import { Hono } from 'hono';

const session = new Hono().get('/session', (c) => c.json({}));

export const auth = new Hono()
  .route('/', session)
  .post('/sign-out', (c) => c.json({}));
