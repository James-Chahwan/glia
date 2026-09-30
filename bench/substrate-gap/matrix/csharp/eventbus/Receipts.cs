using MediatR;

public class SendReceipt : INotificationHandler<OrderPlaced>
{
    public Task Handle(OrderPlaced n, CancellationToken ct) => Task.CompletedTask;
}
