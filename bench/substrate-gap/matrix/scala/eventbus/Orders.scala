package shop

import akka.actor.{ActorRef, ActorSystem}

case class OrderPlaced(id: String)

object Orders {
  def wire(system: ActorSystem, listener: ActorRef): Unit = {
    system.eventStream.subscribe(listener, classOf[OrderPlaced])
    system.eventStream.publish(OrderPlaced("o-1"))
  }
}
