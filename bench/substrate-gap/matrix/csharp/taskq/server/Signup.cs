using Hangfire;

public class Signup
{
    public void Register(string userId)
    {
        BackgroundJob.Enqueue<EmailJobs>(x => x.SendWelcome(userId));
    }
}
