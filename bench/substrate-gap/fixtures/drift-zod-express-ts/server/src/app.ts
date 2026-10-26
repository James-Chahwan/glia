import express from 'express';
import { users } from './routes/users';

const app = express();
app.use(users);
app.listen(3000);
