import { z } from 'zod';

export const UserSchema = z.object({
  id: z.string(),
  displayName: z.string(),
  email_address: z.string().email(),
});

export type User = z.infer<typeof UserSchema>;
