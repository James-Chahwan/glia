using Npgsql;

public class Db
{
    public NpgsqlConnection Open() => new NpgsqlConnection(Environment.GetEnvironmentVariable("DATABASE_URL"));
}
