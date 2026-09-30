using HotChocolate;

public class Query
{
    public IEnumerable<Book> GetBooks() => new List<Book>();
}

public record Book(string Title);
