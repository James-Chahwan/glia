package shop

import akka.actor.{ActorRef, ActorSystem}
import com.typesafe.akka.extension.quartz.QuartzSchedulerExtension

object Jobs {
  def start(system: ActorSystem, purger: ActorRef): Unit =
    QuartzSchedulerExtension(system).schedule("Nightly", purger, "purge")
}
