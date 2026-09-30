package shop

import slick.jdbc.PostgresProfile.api._

class Users(tag: Tag) extends Table[(Long, String)](tag, "users") {
  def id = column[Long]("id", O.PrimaryKey)
  def email = column[String]("email")
  def * = (id, email)
}
