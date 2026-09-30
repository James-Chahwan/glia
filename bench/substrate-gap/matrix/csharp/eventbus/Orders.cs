using MediatR;

public record OrderPlaced(string Id) : INotification;

public class OrderService
{
    private readonly IMediator _mediator;
    public OrderService(IMediator mediator) => _mediator = mediator;

    public Task Place(string id) => _mediator.Publish(new OrderPlaced(id));
}
