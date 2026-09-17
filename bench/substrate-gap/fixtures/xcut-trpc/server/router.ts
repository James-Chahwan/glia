import { z } from "zod";
import { createTRPCRouter, publicProcedure } from "./trpc";
import { db } from "./db";

export const userRouter = createTRPCRouter({
  list: publicProcedure.query(() => db.user.findMany()),
  byId: publicProcedure
    .input(z.object({ id: z.string() }))
    .query(({ input }) => {
      return db.user.findUnique({ where: { id: input.id } });
    }),
});

export const appRouter = createTRPCRouter({
  user: userRouter,
});

export type AppRouter = typeof appRouter;
