package shop

import org.jobrunr.scheduling.BackgroundJob

class Signup(emailJobs: EmailJobs) {
  def register(userId: String): Unit =
    BackgroundJob.enqueue(() => emailJobs.sendWelcome(userId))
}
