using Microsoft.EntityFrameworkCore.Migrations;

public partial class CreateUsers : Migration
{
    protected override void Up(MigrationBuilder migrationBuilder)
    {
        migrationBuilder.CreateTable(
            name: "Users",
            columns: table => new { Id = table.Column<int>(nullable: false) },
            constraints: table => table.PrimaryKey("PK_Users", x => x.Id));
    }
}
