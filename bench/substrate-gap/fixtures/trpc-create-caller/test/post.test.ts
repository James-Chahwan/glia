import { appRouter, createCallerFactory } from "../src/server/router";

test("lists posts", async () => {
  const caller = appRouter.createCaller({});
  const posts = await caller.post.all();
  expect(posts).toEqual([]);
});

test("reads one post through the factory", async () => {
  const createCaller = createCallerFactory(appRouter);
  const api = createCaller({});
  const one = await api.post.byId({ id: "1" });
  const ok = await api.health();
  expect(one.id).toBe("1");
});
