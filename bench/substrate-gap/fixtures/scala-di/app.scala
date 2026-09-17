package app

import com.softwaremill.macwire._

class UserRepo { def find(id: Int): String = "u" }

class Currency

class UserService(repo: UserRepo) { def get(id: Int): String = repo.find(id) }

case class Money(amount: Int, currency: Currency)

object Handlers {
  def render(id: Int)(implicit svc: UserService): String = svc.get(id)
}

object AppWiring {
  lazy val accounts: UserService = wire[UserService]
}
