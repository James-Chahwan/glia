package shop

import org.jobrunr.jobs.annotations.Job

class EmailJobs {
  @Job(name = "send-welcome-email")
  def sendWelcome(userId: String): Unit = ()
}
