package shop

import org.flywaydb.core.Flyway

object Migrate {
  def run(url: String): Unit = Flyway.configure().dataSource(url, "app", "").load().migrate()
}
