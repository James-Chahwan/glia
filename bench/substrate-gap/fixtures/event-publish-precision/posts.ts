// graphql-subscriptions PubSub: an in-process bus, both sides.
import { PubSub } from "graphql-subscriptions";

export const pubsub = new PubSub();

export function announce(post: { id: string }) {
  pubsub.publish("POST_ADDED", { post });
}

export function listen(onPost: (p: unknown) => void) {
  pubsub.subscribe("POST_ADDED", onPost);
}
