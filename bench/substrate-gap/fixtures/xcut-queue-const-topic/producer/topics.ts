// Topic names live in one module and are imported by every publisher: the
// shape that used to collapse each send to `queue_producer:unresolved:kafka`.
export const ORDERS_TOPIC = 'orders';

export const Topics = {
  PAYMENTS: 'payments',
};
