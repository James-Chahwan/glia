using MongoDB.Bson;
using MongoDB.Driver;

namespace Shop.Data;

public class OrderReadModel
{
    public string Id { get; set; } = "";
}

public class OrdersRepository
{
    private readonly IMongoCollection<OrderReadModel> _orders;
    private readonly IMongoCollection<BsonDocument> _audit;
    private readonly IMongoCollection<BsonDocument> _dynamic;

    public OrdersRepository(IMongoDatabase db, string auditName)
    {
        _orders = db.GetCollection<OrderReadModel>(nameof(OrderReadModel));
        _audit = db.GetCollection<BsonDocument>("audit_log");
        _dynamic = db.GetCollection<BsonDocument>(auditName);
    }
}
