import express from 'express';
import items from './routes/items';

const app = express();
const health = express.Router();

health.get('/ping', (req, res) => res.send('ok'));

app.use(express.json());
app.use('/svc/users', require('./routes/users'));
app.use('/svc/items', items);
app.use('/svc/health', health);
app.use('/svc/*', (req, res, next) => next());

app.get('/version', (req, res) => res.send('1'));

app.listen(3000);
