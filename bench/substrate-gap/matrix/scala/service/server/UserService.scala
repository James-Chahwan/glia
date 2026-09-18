import javax.inject._

@Singleton
class UserService @Inject()() {
  def find(id: String): String = id
}
