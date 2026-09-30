# Orders

How orders move through the shop.

## Placing orders

Call `OrderService.place` to place an order.

## Refunds

Refunds are the slowest part of the order lifecycle, and most of the support load comes from them. A customer asks for a refund through the help centre, an agent checks the order history, and the finance team approves anything above the automatic threshold. The threshold is reviewed every quarter and is deliberately conservative, because a refund that is issued twice is very hard to claw back once the card network has settled it. Agents must never issue a refund by hand from the payment provider dashboard, since that bypasses the audit trail the finance team relies on at the end of each month.

Every refund goes through `OrderService.refund`, which records the reason and the approving agent before it calls the payment provider.

A refunded order keeps its Shipment record so that returns can still be traced.
