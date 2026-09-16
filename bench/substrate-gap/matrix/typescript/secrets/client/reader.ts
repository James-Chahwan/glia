import Stripe from 'stripe';

const stripe = new Stripe(process.env.STRIPE_SECRET_KEY as string);

export async function charge(amountCents: number) {
  return stripe.paymentIntents.create({ amount: amountCents, currency: 'usd' });
}
