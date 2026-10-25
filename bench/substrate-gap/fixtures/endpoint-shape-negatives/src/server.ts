import express from 'express';

// A server registration: a route, never a client ENDPOINT.
const app = express();
const routes = express.Router();
routes.get('/health', (req, res) => res.send('ok'));
app.use('/api', routes);

// A mock server definition: an options object with a handler is a route table.
export const mocks = [
  defineMock({ url: '/api/user', method: 'get', response: () => ({ id: 1 }) }),
];

function defineMock(m: { url: string; method: string; response: () => unknown }) {
  return m;
}
