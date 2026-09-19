import { initTRPC } from "@trpc/server";

const t = initTRPC.create();

export const userRouter = t.router({
  list: t.procedure.query(() => []),
});
