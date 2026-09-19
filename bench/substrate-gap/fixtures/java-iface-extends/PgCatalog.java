package shop;

public class PgCatalog implements Catalog {
    public String read(String id) { return id; }
    public String search(String q) { return q; }
}
