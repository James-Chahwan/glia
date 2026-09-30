package shop

object Db {
  val url: String = sys.env("DATABASE_URL")
}
