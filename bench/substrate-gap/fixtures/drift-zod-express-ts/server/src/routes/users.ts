import { Router } from 'express';
import { UserSchema } from '../schemas/user';

export const users = Router();

users.get('/api/users/:id', (req, res) => {
  const user = UserSchema.parse(loadUser(req.params.id));
  res.json(user);
});

function loadUser(id: string): unknown {
  return { id };
}
