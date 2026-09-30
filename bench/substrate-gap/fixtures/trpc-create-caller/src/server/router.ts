import { initTRPC } from "@trpc/server";
import { z } from "zod";

const t = initTRPC.create();

export const postRouter = t.router({
  all: t.procedure.query(() => []),
  byId: t.procedure.input(z.object({ id: z.string() })).query(({ input }) => ({ id: input.id })),
});

export const appRouter = t.router({ post: postRouter, health: t.procedure.query(() => "ok") });
export const createCallerFactory = t.createCallerFactory;
