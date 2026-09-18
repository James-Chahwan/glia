using System.Collections.Generic;
using System.ComponentModel.DataAnnotations.Schema;
using Microsoft.EntityFrameworkCore;

namespace Shop;

public class User { public long Id { get; set; } }

public class Person { public long Id { get; set; } }

[Table("legacy_products")]
public class Product { public long Id { get; set; } }

public class Report { public long Id { get; set; } }

public class AppDbContext : DbContext {
    public DbSet<User> Users { get; set; }
    public DbSet<Person> People => Set<Person>();
    public DbSet<Product>? Products { get; set; }

    protected override void OnModelCreating(ModelBuilder modelBuilder) {
        modelBuilder.Entity<User>().ToTable("app_users");
    }
}

public class ReportBuilder {
    public List<Report> Reports { get; set; }

    public void Configure(ModelBuilder modelBuilder) {
        modelBuilder.Entity<Report>().ToTable("reports");
    }
}
