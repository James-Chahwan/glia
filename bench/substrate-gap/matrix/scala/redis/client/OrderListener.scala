package shop

import dev.profunktor.redis4cats.data.RedisChannel
import dev.profunktor.redis4cats.pubsub.PubSubCommands

object OrderListener {
  def stream[F[_]](pubSub: PubSubCommands[F, fs2.Stream[F, *], String, String]) =
    pubSub.subscribe(RedisChannel("orders"))
}
