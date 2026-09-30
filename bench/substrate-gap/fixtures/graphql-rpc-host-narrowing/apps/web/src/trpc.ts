import { createTRPCProxyClient, httpBatchLink } from "@trpc/client";

export const api = createTRPCProxyClient({ links: [httpBatchLink({ url: "http://catalog-svc/api/trpc" })] });
