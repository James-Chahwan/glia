import express from 'express';
import { UsersService } from './users.service';

const app = express();
const users = new UsersService();

export function listUsers(req: any, res: any) {
  res.json(users.list());
}

app.get('/users', listUsers);
