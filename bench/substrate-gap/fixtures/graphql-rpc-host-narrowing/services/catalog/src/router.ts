import { createTRPCRouter, publicProcedure } from "./trpc";

export const itemRouter = createTRPCRouter({
  list: publicProcedure.query(() => []),
});
