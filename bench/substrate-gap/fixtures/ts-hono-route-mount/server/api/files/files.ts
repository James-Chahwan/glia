import { Hono } from 'hono';

export const filesRoute = new Hono()
  /**
   * Uploads a document file.
   */
  .post('/upload-pdf', async (c) => {
    const body = await c.req.parseBody();
    if (!body.file) {
      return c.json({ error: 'No file provided' }, 400);
    }
    return c.json({ ok: true });
  })
  .get('/item/:id', async (c) => {
    return c.json({ id: c.req.param('id') });
  });
